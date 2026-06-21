---
name: using-git-worktrees
description: Use when starting feature work that needs isolation from the current workspace. This project does NOT use git worktrees — work directly in the main working tree.
---

# Using Git Worktrees

## 本项目不使用 worktree

wjsm 项目**不使用 git worktree**。所有开发工作在单一 working tree 中进行。

当其他 skill（`executing-plans`、`subagent-driven-development`、`brainstorming`、`finishing-a-development-branch`）引用本 skill 时，遵循以下规则：

- **不需要**创建 worktree
- **不需要**验证 `.gitignore` 中的 worktree 目录
- **不需要**执行 worktree 相关的 setup（npm install / cargo build / baseline test）
- 直接在 `~/project/wjsm` 中工作

## 原因

项目规模中等（~16 crate workspace），单 working tree 已足够隔离。`cargo build` target 目录共享节省磁盘与编译时间。`git stash`/`git checkout` 足以处理分支切换。

## 与其他 skill 的集成

本 skill 作为**空操作适配器**存在：满足 `executing-plans` 和 `subagent-driven-development` 对 `aegis:using-git-worktrees` 的 REQUIRED 引用，但不执行实际 worktree 操作。

- **brainstorming**（Phase 4）：跳过 worktree 创建，直接进入实现
- **subagent-driven-development**：跳过 isolated workspace 设置
- **executing-plans**：跳过 isolated workspace 设置
- **finishing-a-development-branch**：跳过 worktree 清理（无 worktree 可清理）
