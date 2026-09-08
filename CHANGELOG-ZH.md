## 0.9.0

- 支持 Godot 4.4–4.7，可将 Rust 用作节点脚本或原生扩展类。
- Rust 脚本支持 Inspector 属性、类型化方法与信号、RPC 和继承，也可向 Godot
  暴露 Rust 常量、枚举和位标志。
- 新项目自动完成 Cargo 初始化，可在 Godot 中管理依赖、查看编译诊断并应用
  编译器修复。支持已有 Cargo 项目、Workspace 和外部编辑器。
- 构建成功后，兼容的代码改动可热重载并保留 Inspector 属性值。更新失败时继续
  使用上一个有效版本；脚本结构变化时需要重新运行场景。
- 通过 Godot 导出窗口发布到 Linux、macOS、Windows、Android、iOS 和 Web，
  需安装对应平台的工具链。
