import { loadDocuments } from "../../lib/content.mjs";
import { loadRootDocuments } from "../../lib/markdown.mjs";

const plainText = (html = "") => html
  .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
  .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
  .replace(/<[^>]*>/g, " ")
  .replace(/&nbsp;/gi, " ")
  .replace(/&amp;/gi, "&")
  .replace(/&lt;/gi, "<")
  .replace(/&gt;/gi, ">")
  .replace(/&quot;/gi, "\"")
  .replace(/&#39;|&apos;/gi, "'")
  .replace(/&#(\d+);/g, (_, value) => String.fromCodePoint(Number(value)))
  .replace(/&[a-z]+;/gi, " ")
  .replace(/\s+/g, " ")
  .trim();

export async function GET() {
  const documents = [
    ...(await loadDocuments("docs")),
    ...(await loadDocuments("spec")),
    ...loadRootDocuments().map((document) => ({ ...document, route: `/project/${document.slug}/`, type: "project" })),
  ];
  const corpus = documents.map((document) => ({
    route: document.route,
    title: document.title,
    summary: document.summary,
    historical: document.type === "historical",
    body: plainText(document.html || ""),
  }));
  return new Response(JSON.stringify(corpus), { headers: { "Content-Type": "application/json; charset=utf-8" } });
}
