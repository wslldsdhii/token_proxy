# Token Proxy 代码库rule

本文件为在 `token_proxy` 项目中工作的 AI 智能体提供指导。

## 项目概览

**Token Proxy** 是基于 Tauri 的 AI API 代理工具，用于转发 OpenAI、Gemini、Anthropic 等 AI API 格式，支持本地运行、token 使用统计、负载均衡和优先级管理。

- 前端: React 19 + TypeScript + Vite + Tailwind CSS v4 + shadcn/ui(pnpm dlx shadcn@latest add xxx)
- 后端: Rust (Edition 2021) + Tokio + Axum
- 桌面框架: Tauri 2

## 参考项目

- 代理转发/转换参考[litellm](.reference/litellm)
- 代理转发/转换参考[new-api](.reference/new-api)
- kiro、codex 等 2api 参考[CLIProxyAPIPlus](.reference/CLIProxyAPIPlus)
- CLIProxyAPIPlus的可视化app参考[quotio](.reference/quotio)

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **token_proxy** (10801 symbols, 31379 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> Index stale? Run `node .gitnexus/run.cjs analyze` from the project root — it auto-selects an available runner. No `.gitnexus/run.cjs` yet? `npx gitnexus analyze` (npm 11 crash → `npm i -g gitnexus`; #1939).

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows. For regression review, compare against the default branch: `detect_changes({scope: "compare", base_ref: "main"})`.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `rename` which understands the call graph.
- NEVER commit changes without running `detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/token_proxy/context` | Codebase overview, check index freshness |
| `gitnexus://repo/token_proxy/clusters` | All functional areas |
| `gitnexus://repo/token_proxy/processes` | All execution flows |
| `gitnexus://repo/token_proxy/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
