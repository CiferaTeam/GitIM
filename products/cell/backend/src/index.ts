import { Hono } from "hono";
import { cors } from "hono/cors";
import type { Bindings } from "./types";
import { inviteRoutes } from "./invite";
import { heartbeatRoutes } from "./heartbeat";
import { adminRoutes } from "./admin";

const app = new Hono<{ Bindings: Bindings }>();

app.use(
  "*",
  cors({
    origin: (origin) => {
      // Allow any localhost port in development
      if (origin.match(/^http:\/\/localhost:\d+$/)) return origin;
      // Allow Cloudflare Pages (production + preview deploys)
      if (origin === "https://cell.gitim.io") return origin;
      if (origin.endsWith(".cell-gitim.pages.dev") || origin === "https://cell-gitim.pages.dev") return origin;
      return null;
    },
    allowMethods: ["GET", "POST", "DELETE"],
    allowHeaders: ["Content-Type", "X-Admin-Secret"],
  })
);

app.route("/", inviteRoutes);
app.route("/", heartbeatRoutes);
app.route("/", adminRoutes);

app.get("/", (c) => c.json({ service: "cell-api", status: "ok" }));

export default app;
