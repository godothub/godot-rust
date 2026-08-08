# godot-rust

[简体中文](README-ZH.md) | [English](README.md)

`godot-rust` 是一套以 Godot 为中心的 Rust 语言支持方案，目标是让开发者像使用
Godot 原生脚本语言一样编写、挂载、调试和导出 Rust 脚本，同时保留 Rust 的性能、
类型安全与 Cargo 生态。

## 特色与优势

- **Godot 优先的工作流**：直接在 Godot 中创建和挂载 `.rs` 脚本，管理项目、依赖、
  构建、诊断和导出，不要求开发者把 Rust 工程当作整个游戏的主入口。
- **贴近 Godot 的脚本体验**：支持生命周期方法、Inspector 导出属性、信号、RPC、
  节点引用、脚本继承、父方法调用、自定义 Resource 和全局类。
- **完整且类型化的 Godot API**：根据 Godot 4.4–4.7 的官方 API 生成类、Builtin、
  Utility、单例、常量、信号和虚方法，并检查每个官方条目的生成或明确归类情况。
- **原生 Cargo 项目**：Godot 项目同时是标准 Cargo 项目，可直接使用 crates.io、
  Workspace、rust-analyzer、Clippy、测试和现有 Rust 工具链。
- **可靠的热重载**：构建成功后自动加载新版本并迁移兼容状态；构建或验证失败时继续
  使用上一个有效版本，避免一次错误中断编辑器工作流。
- **面向交付的安全边界**：隔离脚本 panic，验证模块 ABI、构建产物和官方 API 来源，
  并通过完整 API 覆盖、跨版本测试和多平台导出检查阻止不完整产物进入发布流程。
- **统一的跨平台插件**：一个插件 ZIP 覆盖 Linux、macOS、Windows、Android、iOS
  和 Web，并同时支持 `script` 与 `extension` 两种项目模式。

## Rust 脚本示例

```rust
use godot_rs::prelude::*;

#[script(base = Node)]
pub struct Player {
    #[export(default = 240.0, range(min = 0.0, max = 1000.0))]
    speed: f64,

    #[signal(args(speed))]
    speed_changed: Signal<(f64,)>,
}

#[script]
impl Player {
    pub const MAX_SPEED: f64 = 1000.0;

    fn _ready(&mut self) {
        self.speed_changed.emit((self.speed,));
    }

    #[func(name = "get_double_speed")]
    fn double_speed(&self) -> f64 {
        self.speed * 2.0
    }
}
```

普通字段只属于 Rust；`#[property]` 让字段可由 Godot 读写但不显示在 Inspector，
`#[export]` 再把它显示到 Inspector。公开关联常量会自动成为 Godot 脚本常量，
私有常量保持为 Rust 实现细节。`class_name` 仅在确实需要全局脚本类时填写。
