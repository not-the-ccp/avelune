import fs from "node:fs";
import path from "node:path";
import { marked } from "marked";

const rootDocuments = {
  status: { file: "STATUS.md", title: "Project status", summary: "Implemented scope, interpretation, and known limitations." },
  contributing: { file: "CONTRIBUTING.md", title: "Contributing", summary: "Development workflow and review expectations." },
  security: { file: "SECURITY.md", title: "Security", summary: "Security support and vulnerability reporting." },
  support: { file: "SUPPORT.md", title: "Support", summary: "Where to ask questions and report problems." },
};

const sourceRoutes = new Map([
  ["docs/user/CLI.adoc", "/docs/cli/"],
  ["docs/development/VERSIONING.adoc", "/docs/development/versioning/"],
  ["docs/development/REFERENCE_ORACLE.adoc", "/docs/development/reference-oracle/"],
  ["docs/IPR-NOTES.adoc", "/docs/ipr-notes/"],
  ["spec/", "/spec/"],
  ["spec/README.adoc", "/spec/overview/"],
  ["STATUS.md", "/project/status/"],
  ["CONTRIBUTING.md", "/project/contributing/"],
  ["SECURITY.md", "/project/security/"],
  ["SUPPORT.md", "/project/support/"],
]);

const repositorySourceUrls = new Map([
  ["AGENTS.md", "https://github.com/not-the-ccp/avelune/blob/main/AGENTS.md"],
]);

function canonicalLink(href) {
  if (!href || /^(?:[a-z]+:|#)/i.test(href)) return href;
  const [target, fragment = ""] = href.split("#", 2);
  const route = sourceRoutes.get(target);
  if (route) return `${import.meta.env.BASE_URL.replace(/\/$/, "")}${route}${fragment ? `#${fragment}` : ""}`;
  const sourceUrl = repositorySourceUrls.get(target);
  if (sourceUrl) return `${sourceUrl}${fragment ? `#${fragment}` : ""}`;
  return href;
}

export function loadRootDocuments() {
  return Object.entries(rootDocuments).map(([slug, metadata]) => {
    const source = fs.readFileSync(path.join(process.cwd(), metadata.file), "utf8");
    const renderer = new marked.Renderer();
    const originalLink = renderer.link.bind(renderer);
    renderer.link = ({ href, title, tokens }) => originalLink({ href: canonicalLink(href), title, tokens });
    const tokens = marked.lexer(source);
    // The publication wrapper owns the visible title and source provenance.
    if (tokens[0]?.type === "heading" && tokens[0].depth === 1) tokens.shift();
    return { slug, sourcePath: metadata.file, ...metadata, html: marked.parser(tokens, { renderer }) };
  });
}
