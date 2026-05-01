# wjsm 项目文档生成 Spec

## Why
用户需要一份完整、通俗的项目介绍文档，涵盖项目的规范、结构、执行流程和技术选型等所有方面。当前 README.md 较为简单，AGENTS.md 主要面向开发者，缺少面向用户的综合性介绍文档。

## What Changes
- 创建 `PROJECT_GUIDE.md`，作为项目的综合介绍文档

## Impact
- 新增文件：`PROJECT_GUIDE.md`
- 影响范围：项目文档

## ADDED Requirements
### Requirement: 项目综合介绍文档
文档应包含以下章节：

1. **项目概述** - 项目定位、愿景、核心价值
2. **技术架构** - 完整的编译流水线、各模块职责
3. **目录结构** - 清晰的目录说明
4. **快速开始** - 构建、运行命令
5. **核心概念** - IR 设计、NaN-boxing 值编码、作用域分析
6. **技术栈详解** - Rust、SWC、wasm-encoder、wasmtime
7. **功能现状** - 已实现功能、待实现功能
8. **开发指南** - 添加新功能的流程、测试策略
9. **常见问题**

## MODIFIED Requirements
无

## REMOVED Requirements
无
