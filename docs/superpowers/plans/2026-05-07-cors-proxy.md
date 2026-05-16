# 自托管 CORS Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 提供一个 vendored upstream 逻辑驱动的 Cloudflare Worker Git CORS proxy，并让手机端生产默认可配置。

**Architecture:** `products/cors-proxy/` 独立承载 Worker、测试、vendor 包和部署文档。Worker facade 使用 Fetch API，代理规则来自 `@isomorphic-git/cors-proxy@3.0.1` 的 vendored 代码。前端 Browser Mode 从 `VITE_GIT_CORS_PROXY` 读取默认代理。

**Tech Stack:** Cloudflare Workers module worker、Web Fetch API、Node `node:test`、Vite env。

---

### Task 1: Worker Facade

**Files:**
- Create: `products/cors-proxy/src/cors-proxy.js`
- Create: `products/cors-proxy/src/index.js`
- Create: `products/cors-proxy/tests/cors-proxy.test.js`

- [ ] Write tests for health, CORS, Git GET/POST proxying, host allowlist, redirect rewrite, and OPTIONS.
- [ ] Run `node --test products/cors-proxy/tests/*.test.js` and confirm the module is missing.
- [ ] Implement Worker-compatible facade with the vendored Git request rules.
- [ ] Run `node --test products/cors-proxy/tests/*.test.js` and confirm all tests pass.

### Task 2: Vendor Upstream Package

**Files:**
- Create: `products/cors-proxy/vendor/isomorphic-git-cors-proxy/3.0.1/*`
- Create: `products/cors-proxy/vendor/isomorphic-git-cors-proxy/README.md`

- [ ] Copy the npm tarball contents for `@isomorphic-git/cors-proxy@3.0.1`.
- [ ] Record version, package tarball, integrity, git commit, and license.
- [ ] Keep upstream source files unchanged under `vendor/`.

### Task 3: Deployment Package

**Files:**
- Create: `products/cors-proxy/package.json`
- Create: `products/cors-proxy/wrangler.jsonc`
- Create: `products/cors-proxy/README.md`

- [ ] Add local test and deploy scripts.
- [ ] Add Wrangler JSONC config using `src/index.js` as the Worker entry.
- [ ] Document Cloudflare deployment and production vars.

### Task 4: Frontend Default Proxy

**Files:**
- Modify: `products/cell/frontend/src/components/setup/local-setup.tsx`
- Modify: `products/cell/frontend/.env.example`
- Create or modify: focused frontend test for the default proxy value.

- [ ] Add `VITE_GIT_CORS_PROXY` fallback constant.
- [ ] Use the configured value as the Browser Mode default and input placeholder.
- [ ] Verify the frontend test/build path that covers setup still passes.
