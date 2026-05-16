import { handleCorsProxy } from "./cors-proxy.js";

export default {
  fetch(request, env, ctx) {
    return handleCorsProxy(request, env, ctx);
  },
};
