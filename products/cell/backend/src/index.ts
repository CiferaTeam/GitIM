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
      const allowed = ["https://cell.gitim.io"];
      // Allow any localhost port in development
      if (origin.match(/^http:\/\/localhost:\d+$/)) return origin;
      return allowed.includes(origin) ? origin : null;
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
