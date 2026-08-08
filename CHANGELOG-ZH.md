## 0.9.0

- Script Mode 与 Extension Mode 均可使用按版本生成的 Godot 4.4–4.7 Rust SDK，覆盖类、Builtin、Utility、单例、常量、信号和虚方法。
- Rust 脚本支持类型化 Inspector/隐藏属性、自定义访问器、方法、信号、RPC、继承、父方法调用、稳定 Resource UID 和编译期检查。
- 普通 Rust 公开常量以及可向前兼容的自定义枚举和位标志可直接提供给 Godot，无需重复声明。
- 构建成功后无需重启编辑器即可热重载并保留兼容状态；更新失败时继续使用上一个有效版本。
- 可直接在 Godot 中自动初始化并管理 Cargo 项目、Workspace、依赖、诊断、编译器修复、Rust Target、安全模式和外部编辑器。
- Script Mode 与 Extension Mode 均可构建并导出到 Linux、macOS、Windows、Android、iOS 和 Web。
- 可通过 Cargo 安装 SDK，并且只为选中的 Godot 4.4–4.7 API 生成绑定。
