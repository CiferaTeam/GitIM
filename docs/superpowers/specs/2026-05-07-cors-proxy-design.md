# 自托管 CORS Proxy 设计

## 目标

为浏览器模式提供一个可部署到 Cloudflare Workers 的 Git CORS proxy。服务兼容 isomorphic-git 的 `corsProxy` 请求格式，生产环境由 GitIM 自托管，不依赖公共 `cors.isomorphic-git.org`。

## 架构

`products/cors-proxy/` 是独立 Worker 包。仓库 vendor `@isomorphic-git/cors-proxy@3.0.1` 作为代理规则来源，并在 `src/` 中提供 Cloudflare Worker facade。facade 负责 Worker `fetch` 入口、CORS origin 选择、允许的 upstream host 配置、健康检查和部署配置。

浏览器端通过 `VITE_GIT_CORS_PROXY` 设置默认代理地址。用户本地保存过的代理地址继续优先使用。

## 配置

- `ALLOW_ORIGINS`：逗号分隔的允许来源，默认 `*`。
- `ALLOWED_HOSTS`：逗号分隔的 upstream host，默认 `github.com`。
- `INSECURE_HTTP_ORIGINS`：开发用 HTTP upstream host 列表。

## 数据流

1. 手机端 isomorphic-git 请求 `https://<proxy>/<host>/<repo-path>/info/refs?service=git-upload-pack`。
2. Worker facade 处理 CORS 和健康检查。
3. facade 复用 vendored upstream 的 Git 请求规则判断请求是否可代理。
4. 通过 Worker `fetch` 转发到 upstream Git host。
5. 返回 upstream 响应，并暴露 Git 客户端需要读取的 headers。

## 验证

- Node 内置测试覆盖 Worker facade 的 GET、POST、OPTIONS、host allowlist、CORS 和 redirect header 行为。
- 前端单元测试验证环境变量会影响 Browser Mode 默认 CORS proxy。
