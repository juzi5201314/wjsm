# Tasks
- [x] Task 1: 扩展解析器以识别 ES 模块声明
  - [x] SubTask 1.1: 在 AST/语法解析中加入 `ImportDecl`、`ExportDecl`、`ExportDefault` 的最小结构
  - [x] SubTask 1.2: 为 `import {}`、`import default`、`export const`、`export {}`、`export default` 增加解析用例

- [x] Task 2: 语义层实现模块符号与依赖图
  - [x] SubTask 2.1: 收集每个模块的导出表并建立导入绑定关系
  - [x] SubTask 2.2: 对缺失导出、重复导出、重复导入别名给出语义错误
  - [x] SubTask 2.3: 产出可供运行时消费的模块依赖图/链接结果

- [x] Task 3: 运行时实现模块加载、缓存与执行
  - [x] SubTask 3.1: 按模块图顺序加载并实例化模块环境
  - [x] SubTask 3.2: 增加模块缓存，确保同一模块仅执行一次
  - [x] SubTask 3.3: 在基础循环依赖场景下保持可执行并返回可预测行为

- [ ] Task 4: 接入 CLI 与模块 fixtures
  - [ ] SubTask 4.1: 入口执行流程支持模块模式分支
  - [ ] SubTask 4.2: 新增 `fixtures/modules` 的命名导出、默认导出、多模块复用示例
  - [ ] SubTask 4.3: 补齐集成测试与期望输出

- [ ] Task 5: 验证与回归检查
  - [ ] SubTask 5.1: 运行相关单元/集成测试并修复失败项
  - [ ] SubTask 5.2: 覆盖错误路径（导入不存在导出）并确认报错信息

# Task Dependencies
- Task 2 depends on Task 1
- Task 3 depends on Task 2
- Task 4 depends on Task 3
- Task 5 depends on Task 4
