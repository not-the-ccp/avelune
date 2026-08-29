import { loadDocuments } from "../../lib/content.mjs";
import { loadRootDocuments } from "../../lib/markdown.mjs";

const searchableText = (document) =>
  `${document.title} ${document.summary} ${document.type} ${document.html || ""}`
    .replace(/<[^>]*>/g, " ")
    .replace(/&(?:[a-z]+|#\d+);/gi, " ")
    .replace(/\s+/g, " ")
    .toLocaleLowerCase();

export async function GET() {
  const documents = [
    ...(await loadDocuments("docs")),
    ...(await loadDocuments("spec")),
    ...loadRootDocuments().map((document) => ({ ...document, route: `/project/${document.slug}/`, type: "project" })),
  ];
  const corpus = documents.map((document) => ({ route: document.route, text: searchableText(document) }));
  return new Response(JSON.stringify(corpus), { headers: { "Content-Type": "application/json; charset=utf-8" } });
}