# godot-rust

[English](README.md) | 简体中文

**在 Godot 里，用 Rust 写游戏。**

像使用 Godot 脚本一样创建和挂载 `.rs` 文件，Godot 用户友好的代码风格，通过 Godot 导出游戏。熟悉的编辑器工作流，加上 Rust 的性能、类型安全和 Cargo 生态。

## 为什么选择 godot-rust

- **融入 Godot**：通过编辑器创建和挂载 Rust 脚本，更可读、面向 Godot
  用户的代码风格和语法糖。
- **类型安全的引擎 API**：在 Rust 中调用 Godot 类、单例和内置类型，
  用编译检查发现类型错误。
- **支持热重载**：修改代码并构建，兼容的更新可保留 Inspector 属性值；
  构建失败时继续使用上一个可用版本。
- **错误就在编辑器里**：保存后自动检查，错误和警告显示在 Rust 面板，
  双击即可跳转到源码。
- **沿用 Rust 工具链**：使用 crates.io 依赖、Cargo Workspace、
  rust-analyzer 和 Clippy，也可以在熟悉的外部编辑器中编写代码。

## 快速开始

准备好 Godot 4.4+、Rust 1.85+ 和本机编译工具链。

### 1. 安装插件

从 [Releases](https://github.com/godothub/godot-rust/releases) 下载插件 ZIP，
解压到 Godot 项目根目录，确认存在：

```text
res://addons/godot-rust/plugin.cfg
```

打开项目，在 **项目 > 项目设置 > 插件** 中启用 **godot-rust**。
新项目会自动创建并配置 Cargo 项目；已有 Cargo 项目按编辑器提示完成配置。

### 2. 创建 Rust 脚本

选中场景节点，点击 **挂载脚本**，将语言设为 **Rust**，
保存到 `res://src/scripts/` 下。插件会自动管理脚本的模块声明。

例如，给 `Node2D` 挂载下面的脚本，即可在 Inspector 中调整水平移动速度：

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player {
    #[export(default = 240.0)]
    speed: f64,
}

#[script]
impl Player {
    fn _ready(&mut self) -> EngineResult<()> {
        self.base().set_process(true)?;
        Ok(())
    }

    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let offset = Vector2::new((self.speed * delta) as f32, 0.0);
        self.base().translate(offset)?;
        Ok(())
    }
}
```

`#[script]` 声明脚本及其回调，`#[export]` 将字段显示到 Inspector，
`self.base()` 用于操作挂载脚本的节点。

### 3. 运行与迭代

按 **F5** 运行项目，或按 **F6** 运行当前场景；插件会在需要时自动构建 Rust 代码。
也可以点击底部 **Rust > Build**，或使用 **项目 > 工具 > Rust: Build** 手动构建。

保存文件后会自动检查代码；要应用修改，再执行一次 **Build**。
兼容的修改支持热重载，字段类型、方法签名或继承关系变化时需要重新运行场景。

## 导出游戏

使用 Godot 的 **项目 > 导出**，插件会按导出预设构建并打包 Rust 代码。
同一个插件 ZIP 包含支持平台的运行组件，各平台仍需安装对应工具链和 Godot 导出模板。

| 平台 | 环境要求 |
| --- | --- |
| Windows、Linux、macOS | 在对应操作系统上导出；macOS 需要 Xcode。 |
| Android | Android SDK、NDK 和对应 Rust Target。 |
| iOS | macOS 与 Xcode，支持真机和模拟器。 |
| Web | Emscripten 和匹配的 Godot 动态扩展模板。 |
