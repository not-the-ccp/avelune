import fs from "node:fs";
import path from "node:path";
import { load } from "@asciidoctor/core";

const repositoryRoot = process.cwd();

export const contentRoots = {
  docs: path.join(repositoryRoot, "docs"),
  spec: path.join(repositoryRoot, "spec"),
};

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(full) : [full];
  });
}

/** Return the stable publication route for an authoritative source file. */
export function sourceRoute(kind, relativePath) {
  const source = relativePath.replaceAll("\\", "/").replace(/^\.\//, "");
  if (source === "docs/user/CLI.adoc") return "/docs/cli/";
  if (source === "spec/README.adoc") return "/spec/overview/";
  if (source === "spec/CHANGELOG.adoc") return "/spec/changelog/";
  if (source === "spec/001-v1-baseline-profile.adoc") return "/spec/baseline/";
  const specDocument = source.match(/^spec\/(video|audio|container|common|conformance)\/(00[01])-(.+)\.adoc$/);
  if (specDocument) {
    const [, area, generation, name] = specDocument;
    if (generation === "000") return `/spec/history/${name === "requirements" ? `${area}-candidate` : name}/`;
    const currentNames = { video: "alv1", audio: "ala1", container: "avl", common: "entropy", conformance: "generation-1" };
    return `/spec/${area}/${currentNames[area]}/`;
  }
  const root = `${kind}/`;
  const remainder = source.startsWith(root) ? source.slice(root.length) : source;
  const slug = remainder.replace(/\.adoc$/, "").replaceAll("_", "-").toLowerCase();
  return `/${kind}/${slug}/`;
}

function baseUrl() {
  // Keep direct Node consumers deterministic while Astro supplies its configured base at build time.
  return (import.meta.env?.BASE_URL || "/avelune/").replace(/\/$/, "");
}

export function routeWithBase(route) {
  return `${baseUrl()}${route.startsWith("/") ? route : `/${route}`}`;
}

function collectAnchors(document) {
  return document.findBy({ context: "section" })
    .filter((section) => section.getLevel() > 0 && section.getId())
    .map((section) => ({
      depth: section.getLevel(),
      id: section.getId(),
      // Asciidoctor returns converted inline markup here; the outline is plain text.
      text: section.getTitle().replace(/<[^>]+>/g, ""),
    }));
}

function sourcePathForTarget(sourcePath, target) {
  const filePart = target.split("#", 1)[0];
  if (!filePart || !filePart.endsWith(".adoc")) return null;
  const sourceFile = path.resolve(repositoryRoot, sourcePath);
  const resolved = path.resolve(path.dirname(sourceFile), filePart);
  const relative = path.relative(repositoryRoot, resolved).replaceAll("\\", "/");
  if (relative.startsWith("../") || !/^(docs|spec)\/.+\.adoc$/.test(relative)) return null;
  return relative;
}

/**
 * Convert inter-document source xrefs to publication URLs before Asciidoctor renders them.
 * This intentionally operates on an in-memory rendering copy only; checked-in AsciiDoc is
 * still the authoritative text and xref syntax remains the authoring contract.
 */
export function canonicalizeSourceXrefs(sourcePath, source) {
  let delimiter = null;
  return source.split(/\r?\n/).map((line) => {
    const marker = line.trim();
    if (/^(----|\.\.\.\.|\+\+\+\+)$/.test(marker)) {
      delimiter = delimiter === marker ? null : (delimiter || marker);
      return line;
    }
    if (delimiter) return line;

    return line.replace(/(^|[^\\\w])xref:([^\s\[]+)(\[[^\]]*\])/g, (whole, prefix, target, label) => {
      const targetPath = sourcePathForTarget(sourcePath, target);
      if (!targetPath) return whole;
      const targetKind = targetPath.split("/", 1)[0];
      const fragment = target.includes("#") ? `#${target.split("#").slice(1).join("#")}` : "";
      return `${prefix}link:${routeWithBase(sourceRoute(targetKind, targetPath))}${fragment}${label}`;
    });
  }).join("\n");
}

async function loadDocument(file, kind) {
  const sourcePath = path.relative(repositoryRoot, file).replaceAll("\\", "/");
  const source = canonicalizeSourceXrefs(sourcePath, fs.readFileSync(file, "utf8"));
  const document = await load(source, {
    safe: "safe",
    base_dir: repositoryRoot,
    sourcemap: true,
    attributes: {
      "source-highlighter": "highlight.js",
    },
  });
  const route = sourceRoute(kind, sourcePath);
  const toc = collectAnchors(document);
  return {
    kind,
    sourcePath,
    route,
    slug: route.slice(`/${kind}/`.length, -1),
    title: document.getDocumentTitle(),
    summary: document.getAttribute("summary") || "",
    status: document.getAttribute("page-status") || "",
    type: document.getAttribute("page-type") || "guide",
    order: Number(document.getAttribute("nav-order") || 999),
    // `anchors` predates the right-hand outline; retain it for consumers of this module.
    anchors: toc,
    toc,
    html: await document.convert({ to_file: false }),
  };
}

export async function loadDocuments(kind) {
  const root = contentRoots[kind];
  if (!fs.existsSync(root)) return [];
  const documents = await Promise.all(walk(root)
    .filter((file) => file.endsWith(".adoc"))
    .map((file) => loadDocument(file, kind)));
  const seenRoutes = new Map();
  for (const document of documents) {
    if (seenRoutes.has(document.route)) throw new Error(`Duplicate publication route ${document.route}: ${seenRoutes.get(document.route)} and ${document.sourcePath}`);
    seenRoutes.set(document.route, document.sourcePath);
  }
  return documents.sort((a, b) => a.order - b.order || a.title.localeCompare(b.title));
}
