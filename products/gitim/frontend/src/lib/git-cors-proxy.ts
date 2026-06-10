export const PUBLIC_GIT_CORS_PROXY = "https://git-cors.gitim.io";
export const DEFAULT_GIT_CORS_PROXY =
  import.meta.env.VITE_GIT_CORS_PROXY?.trim() || PUBLIC_GIT_CORS_PROXY;
