# GitIM CORS Proxy

这是 GitIM 浏览器模式使用的自托管 Git CORS proxy，部署目标是 Cloudflare Workers。

核心 Git 请求规则来自 vendored `@isomorphic-git/cors-proxy@3.0.1`。`src/` 只提供 Cloudflare Worker Fetch API facade、健康检查和项目级配置。

## 配置

Wrangler 变量：

- `ALLOW_ORIGINS`：逗号分隔的前端来源，生产默认 `https://gitim.io,https://www.gitim.io`。
- `ALLOWED_HOSTS`：逗号分隔的 Git upstream host，生产默认 `github.com`。
- `INSECURE_HTTP_ORIGINS`：本地开发用 HTTP upstream host 列表。

## 本地验证

```sh
npm install
npm test
npm run check
```

## 部署

```sh
npm install
npm run deploy
```

部署后，把 Worker 地址写入前端生产环境：

```sh
VITE_GIT_CORS_PROXY=https://gitim-cors-proxy.<account>.workers.dev
```

## 健康检查

```sh
curl https://gitim-cors-proxy.<account>.workers.dev/health
```

期望返回：

```json
{"ok":true,"service":"gitim-cors-proxy"}
```
