#!/usr/bin/env node
/** Validate the restricted AsciiDoc publication profile before Astro renders it. */

import fs from "node:fs";
import path from "node:path";
import { load } from "@asciidoctor/core";

const root = process.cwd();
const allowedTypes = new Set(["guide", "architecture", "development-guide", "normative", "supporting", "historical"]);
const requiredAttributes = ["page-type", "page-status", "summary", "nav-order"];
const normativeStatus = "current normative draft · unfrozen";
const historicalStatus = "historical · non-normative";
const allowedNormativeAttributes = new Set([...requiredAttributes, "sectnums"]);

function walk(directory) {
  if (!fs.existsSync(directory)) return [];
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(full) : [full];
  });
}

function location(document, line) {
  return `${document.sourcePath}:${line}`;
}

function isDelimiter(line) {
  const marker = line.trim();
  return /^(----|\.\.\.\.|\+\+\+\+|====)$/.test(marker) ? marker : null;
}

function authoredLines(content) {
  let delimiter = null;
  return content.split(/\r?\n/).map((text, index) => {
    const marker = isDelimiter(text);
    const inDelimitedBlock = Boolean(delimiter);
    if (marker) delimiter = delimiter === marker ? null : (delimiter || marker);
    return { text, line: index + 1, inDelimitedBlock: inDelimitedBlock || Boolean(marker) };
  });
}

function parseHeader(document, errors) {
  const lines = document.content.split(/\r?\n/);
  let index = 0;
  while (index < lines.length && !lines[index].trim()) index += 1;
  if (!/^=\s+\S/.test(lines[index] || "")) {
    errors.push(`${location(document, index + 1)}: document must begin with a level-zero title`);
  }
  index += 1;
  const attributes = new Map();
  for (; index < lines.length && lines[index].trim(); index += 1) {
    const match = lines[index].match(/^:([\w-]+):\s*(.*)$/);
    if (!match) continue;
    const [, name, value] = match;
    if (attributes.has(name)) errors.push(`${location(document, index + 1)}: duplicate document attribute :${name}:`);
    attributes.set(name, value);
  }
  document.attributes = attributes;
}

function parseIdentifiers(document, errors) {
  const lines = authoredLines(document.content);
  const ids = new Map();
  const headingIds = new Map();
  for (let index = 0; index < lines.length; index += 1) {
    const current = lines[index];
    if (current.inDelimitedBlock) continue;
    const anchor = current.text.match(/^\s*\[#([^\]]+)\]\s*$/);
    if (anchor) {
      const id = anchor[1];
      if (ids.has(id)) {
        errors.push(`${location(document, current.line)}: duplicate ID #${id} (first declared at line ${ids.get(id)})`);
      } else {
        ids.set(id, current.line);
      }
    }
    const heading = current.text.match(/^(={2,})\s+\S/);
    if (heading) {
      const previous = lines[index - 1];
      const id = previous && !previous.inDelimitedBlock ? previous.text.match(/^\s*\[#([^\]]+)\]\s*$/)?.[1] : null;
      if (id) headingIds.set(current.line, id);
      if (document.attributes.get("page-type") === "normative" && !id) {
        errors.push(`${location(document, current.line)}: every normative section needs an explicit [#semantic-id] immediately before its heading`);
      }
    }
  }
  document.ids = ids;
  document.headingIds = headingIds;
}

function isHistoricalPath(sourcePath) {
  const relative = sourcePath.replace(/^(docs|spec)\//, "");
  return /(?:^|\/)history\//.test(relative) || /(?:^|\/)000-[^/]+\.adoc$/.test(relative);
}

function validateMetadata(document, errors) {
  const attributes = document.attributes;
  for (const name of requiredAttributes) {
    if (!attributes.get(name)?.trim()) errors.push(`${document.sourcePath}: missing required :${name}: metadata`);
  }
  const type = attributes.get("page-type");
  if (type && !allowedTypes.has(type)) errors.push(`${document.sourcePath}: unsupported page-type ${JSON.stringify(type)}`);
  const order = attributes.get("nav-order");
  if (order && !/^-?\d+$/.test(order)) errors.push(`${document.sourcePath}: :nav-order: must be an integer`);
  const status = attributes.get("page-status");
  if (type === "normative" && status !== normativeStatus) {
    errors.push(`${document.sourcePath}: normative pages require :page-status: ${normativeStatus}`);
  }
  if (type === "historical" && status !== historicalStatus) {
    errors.push(`${document.sourcePath}: historical pages require :page-status: ${historicalStatus}`);
  }
  if (type && type !== "normative" && type !== "historical" && status && status !== "current") {
    errors.push(`${document.sourcePath}: non-normative, non-historical pages require :page-status: current`);
  }
  if (isHistoricalPath(document.sourcePath) && type !== "historical") {
    errors.push(`${document.sourcePath}: historical candidate material must use :page-type: historical`);
  }
  if (type === "normative" && isHistoricalPath(document.sourcePath)) {
    errors.push(`${document.sourcePath}: historical candidate material cannot be normative`);
  }
}

function localPath(document, rawTarget) {
  if (!rawTarget || /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(rawTarget)) return null;
  const target = path.resolve(path.dirname(document.absolutePath), rawTarget);
  const relative = path.relative(root, target);
  return relative.startsWith("..") || path.isAbsolute(relative) ? null : target;
}

function xrefParts(target) {
  const marker = target.indexOf("#");
  return marker < 0 ? { file: target, fragment: "" } : { file: target.slice(0, marker), fragment: target.slice(marker + 1) };
}

function validateReferences(documents, errors) {
  const byPath = new Map(documents.map((document) => [document.sourcePath, document]));
  for (const document of documents) {
    for (const entry of authoredLines(document.content)) {
      if (entry.inDelimitedBlock) continue;
      for (const match of entry.text.matchAll(/\bxref:([^\s\[]+)\[[^\]]*\]/g)) {
        const target = match[1];
        let { file, fragment } = xrefParts(target);
        if (file.endsWith(".md")) {
          errors.push(`${location(document, entry.line)}: xref points at stale Markdown source ${file}`);
          continue;
        }
        if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(file)) continue;
        let destination = document;
        if (file) {
          // Asciidoctor also permits xref:local-anchor[] as shorthand for #local-anchor.
          if (!fragment && !file.includes("/") && !path.extname(file) && document.ids.has(file)) {
            fragment = file;
            file = "";
          }
        }
        if (file) {
          const targetPath = localPath(document, file);
          if (!targetPath || path.extname(file) !== ".adoc") {
            errors.push(`${location(document, entry.line)}: xref target must be a local .adoc source or fragment (${target})`);
            continue;
          }
          const sourcePath = path.relative(root, targetPath).replaceAll("\\", "/");
          destination = byPath.get(sourcePath);
          if (!destination) {
            errors.push(`${location(document, entry.line)}: xref target does not exist: ${file}`);
            continue;
          }
        }
        if (fragment && !destination.ids.has(fragment)) {
          errors.push(`${location(document, entry.line)}: xref fragment #${fragment} does not exist in ${destination.sourcePath}`);
        }
      }
      for (const match of entry.text.matchAll(/\b(?:xref|link):([^\s\[]+\.md(?:#[^\s\[]*)?)\[[^\]]*\]/g)) {
        if (!/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(match[1])) {
          errors.push(`${location(document, entry.line)}: link points at stale Markdown source ${match[1]}`);
        }
      }
      for (const match of entry.text.matchAll(/\bimage::?([^\s\[]+)\[[^\]]*\]/g)) {
        const asset = match[1];
        const target = localPath(document, asset);
        if (!target) {
          if (!/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(asset)) {
            errors.push(`${location(document, entry.line)}: local asset must stay inside the repository: ${asset}`);
          }
          continue;
        }
        if (!fs.existsSync(target) || !fs.statSync(target).isFile()) {
          errors.push(`${location(document, entry.line)}: local asset does not exist: ${asset}`);
        }
      }
    }
  }
}

function validateNormativeRestrictions(document, errors) {
  if (document.attributes.get("page-type") !== "normative") return;
  for (const [name] of document.attributes) {
    if (!allowedNormativeAttributes.has(name)) {
      errors.push(`${document.sourcePath}: normative document uses forbidden attribute :${name}:`);
    }
  }
  for (const entry of authoredLines(document.content)) {
    if (entry.inDelimitedBlock) continue;
    const text = entry.text;
    if (/^\s*include::/.test(text)) errors.push(`${location(document, entry.line)}: normative content must not use include::`);
    if (/^\s*(?:ifdef|ifndef|ifeval|endif)::/.test(text)) errors.push(`${location(document, entry.line)}: normative content must not use conditional directives`);
    if (/pass:\[|\+\+\+/.test(text)) errors.push(`${location(document, entry.line)}: normative content must not use passthrough HTML`);
    if (/<\/?[A-Za-z][^>]*>/.test(text)) errors.push(`${location(document, entry.line)}: normative content must not contain raw HTML`);
    if (/^\s*\[\.|\brole\s*=/.test(text)) errors.push(`${location(document, entry.line)}: normative content must not use presentation roles or CSS classes`);
    if (!/^:[\w-]+:/.test(text) && /\{[A-Za-z][\w-]*\}/.test(text)) {
      errors.push(`${location(document, entry.line)}: normative content must not hide text through attribute substitution`);
    }
  }
}

async function validateAsciidoctor(document, errors) {
  try {
    await load(document.content, { safe: "safe", base_dir: root, sourcemap: true });
  } catch (error) {
    errors.push(`${document.sourcePath}: Asciidoctor safe-mode parse failed: ${error.message}`);
  }
}

const documents = walk(path.join(root, "docs")).concat(walk(path.join(root, "spec")))
  .filter((file) => file.endsWith(".adoc"))
  .sort()
  .map((absolutePath) => ({
    absolutePath,
    sourcePath: path.relative(root, absolutePath).replaceAll("\\", "/"),
    content: fs.readFileSync(absolutePath, "utf8"),
    attributes: new Map(),
    ids: new Map(),
  }));

const errors = [];
for (const document of documents) {
  parseHeader(document, errors);
  parseIdentifiers(document, errors);
  validateMetadata(document, errors);
  validateNormativeRestrictions(document, errors);
}

const normativeIds = new Map();
for (const document of documents.filter((candidate) => candidate.attributes.get("page-type") === "normative")) {
  for (const [id, line] of document.ids) {
    if (!/^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(id)) {
      errors.push(`${location(document, line)}: normative ID #${id} must be lowercase ASCII and include a domain prefix`);
    }
    const existing = normativeIds.get(id);
    if (existing) errors.push(`${location(document, line)}: normative ID #${id} is already declared at ${existing}`);
    else normativeIds.set(id, location(document, line));
  }
}

validateReferences(documents, errors);
await Promise.all(documents.map((document) => validateAsciidoctor(document, errors)));

const { sourceRoute } = await import("../site/src/lib/content.mjs");
const publicationRoutes = new Map();
for (const document of documents) {
  const kind = document.sourcePath.split("/", 1)[0];
  const route = sourceRoute(kind, document.sourcePath);
  const existing = publicationRoutes.get(route);
  if (existing) errors.push(`${document.sourcePath}: publication route ${route} is already assigned to ${existing}`);
  else publicationRoutes.set(route, document.sourcePath);
}

if (errors.length) {
  for (const error of errors) console.error(`content validation: ${error}`);
  console.error(`content validation FAILED (${errors.length} error${errors.length === 1 ? "" : "s"})`);
  process.exitCode = 1;
} else {
  console.log(`content validation PASS (${documents.length} AsciiDoc document${documents.length === 1 ? "" : "s"})`);
}
