import { Hono } from "hono";
import { cors } from "hono/cors";
import type { Bindings } from "./types";
import { adminRoutes } from "./admin";
import { versionRoutes } from "./version";
import { statsRoutes } from "./stats";

const app = new Hono<{ Bindings: Bindings }>();

app.use(
  "*",
  cors({
    origin: (origin) => {
      // Allow any localhost port in development
      if (origin.match(/^http:\/\/localhost:\d+$/)) return origin;
      // Allow Cloudflare Pages (production + preview deploys)
      if (origin === "https://gitim.io" || origin === "https://www.gitim.io") return origin;
      if (origin.endsWith(".gitim.pages.dev") || origin === "https://gitim.pages.dev") return origin;
      return null;
    },
    allowMethods: ["GET", "POST", "DELETE"],
    allowHeaders: ["Content-Type", "X-Admin-Secret"],
  })
);

app.route("/", adminRoutes);
app.route("/", versionRoutes);
app.route("/", statsRoutes);

app.get("/", (c) => c.json({ service: "gitim-api", status: "ok" }));

export default app;
