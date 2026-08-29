import { loadDocuments } from "../lib/content.mjs";
import { loadRootDocuments } from "../lib/markdown.mjs";

export async function GET() {
  const site = new URL(import.meta.env.BASE_URL, import.meta.env.SITE);
  const origin = `${site.origin}${site.pathname.replace(/\/$/, "")}`;
  const routes = ["/", "/docs/", "/spec/", "/api/", "/demo/", "/search/",
    ...(await loadDocuments("docs")).map((document) => document.route),
    ...(await loadDocuments("spec")).map((document) => document.route),
    ...loadRootDocuments().map((document) => `/project/${document.slug}/`),
  ];
  const body = `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${[...new Set(routes)].map((route) => `  <url><loc>${origin}${route}</loc></url>`).join("\n")}\n</urlset>\n`;
  return new Response(body, { headers: { "Content-Type": "application/xml; charset=utf-8" } });
}
