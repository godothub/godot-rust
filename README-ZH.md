# godot-rust

[English](README.md) | 简体中文

Godot 4.4–4.7 · Rust 1.85+ · MIT

`godot-rust` 是一套以 Godot 为中心的 Rust 语言支持方案。它让开发者能够像使用
Godot 原生脚本语言一样创建、挂载、编辑、诊断、热重载和导出 Rust 脚本，同时
保留 Rust 的性能、类型安全与 Cargo 生态。

日常工作从 Godot 开始，而不是从一个 Rust 应用程序开始。Godot 项目同时是标准
Cargo 项目，`.rs` 文件是真正的 Script 资源，编辑器插件负责协调工具链和运行时
边界。

## 项目特色

- **Godot 优先的工作流**——通过 Godot 自带的脚本窗口创建和挂载 `.rs` 文件；
  在编辑器内完成 Cargo 配置、检查、构建、诊断、依赖管理、运行和导出。
- **完整生成的 Godot API**——根据 Godot 4.4–4.7 官方 API 生成类、Builtin、
  Utility、单例、常量、信号和虚方法。覆盖门槛要求每个官方条目都必须生成或
  明确归类。
- **可靠的热重载**——兼容的 Script 状态以事务方式迁移；构建失败或候选模块
  不兼容时继续运行上一个有效版本。`extension` 类也支持恢复属性的重载流程。
- **可执行的错误诊断**——Cargo 信息显示在编辑器底部 Rust 面板中，支持源码
  跳转和预览编译器修复。Panic 被限制在 ABI 边界内，只停用发生问题的脚本实例。
- **标准 Rust 工具链**——继续使用 crates.io、Cargo Workspace、rust-analyzer、
  Clippy、测试、CodeLLDB 和 GDB。
- **单一跨平台插件**——一个插件 ZIP 同时包含桌面编辑器 Host，以及桌面、
  Android、iOS 和 Web 导出所需的运行组件。

## 快速开始

### 1. 安装插件

从 [Releases](https://github.com/godothub/godot-rust/releases) 下载插件
ZIP，并解压到 Godot 项目根目录。最终路径必须是：

```text
res://addons/godot-rust/plugin.cfg
```

使用 Godot 打开项目，然后在以下位置启用 **godot-rust**：

```text
项目 > 项目设置 > 插件
```

ZIP 已包含各编辑器平台所需的二进制文件，用户无需根据操作系统下载不同插件。

### 2. 配置 Cargo 项目

项目不存在 `Cargo.toml` 时，插件会直接自动执行以下命令，无需弹窗确认或手工操作：

```sh
cargo init --lib --vcs none --name godothub_project .
```

随后自动添加兼容的 `godot_rs` 依赖、库目标、`script` 模式配置、生成的脚本入口、
Cargo API 选择和编辑器集成。插件不会为 `cargo init` 强制指定 Rust Edition，
不会覆盖已有清单，也不会替换已有 `src/lib.rs`。

已有 Cargo 包和 Workspace 会在原结构上配置。遇到冲突或无法确定目标包时，插件
会在修改任何文件之前停止并要求用户选择。

### 3. 创建 Rust 脚本

1. 在场景树中选择节点，点击 Godot 自带的“挂载脚本”按钮。
2. 把语言切换为 **Rust**。
3. 将脚本保存到 `res://src/scripts/` 下。
4. 点击“创建”，插件会生成 Rust 类型和稳定的 Resource UID。

生成的脚本使用普通 Rust，只在 Godot 需要元数据时添加标记：

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player {
    #[export(default = 240.0, range(min = 0.0, max = 1000.0))]
    speed: f64,

    #[signal(args(speed))]
    speed_changed: Signal<(f64,)>,
}

#[script]
impl Player {
    pub const MAX_SPEED: f64 = 1000.0;

    fn _ready(&mut self) -> EngineResult<()> {
        self.base().set_process(true)?;
        godot_print!("Player is ready");
        Ok(())
    }

    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let offset = Vector2::new(self.speed * delta, 0.0);
        self.base().translate(offset)?;
        Ok(())
    }

    #[func(name = "double_speed")]
    fn doubled_speed(&self) -> f64 {
        self.speed * 2.0
    }
}
```

### 4. 构建和运行

使用顶部 **项目 > 工具 > Rust: Build**，或编辑器底部 Rust 面板中的 **Build**。
只有通过独立校验的构建才会成为当前项目模块。按 F5/F6 时，插件会检查经过内容
验证的构建记录；仅当 Rust 输入或工具链证据发生变化时才执行增量构建。

默认情况下，保存 Rust 文件会触发合并延迟后的 Cargo Check。错误和警告会显示
在 Rust 面板中，双击诊断即可打开对应 `.rs` 文件和行列。

## 脚本模型

| Rust 写法 | Godot 行为 |
| --- | --- |
| `#[script(base = Node2D)]` | 声明可挂载的 Rust Script 及其 Godot 基类。 |
| `#[script] impl Player` | 收集这个 impl 中的生命周期回调和反射成员。 |
| `_ready`、`_process`、`_input` 等 | 自动注册签名正确的生命周期回调。 |
| `#[export(...)]` | 创建在 Inspector 中显示并保存的属性。 |
| `#[property(...)]` | 创建不显示在 Inspector 中的 Godot 属性。 |
| `#[node("%Name")]` | 在 `_ready` 前解析并检查场景节点类型。 |
| `#[signal(args(...))]` | 声明类型化 Godot 信号。 |
| `#[func]` | 向 Godot 暴露类型化方法。 |
| `#[rpc(...)]` | 使用经过校验的 Godot RPC 元数据暴露方法。 |
| `pub const` | 将编译产物中的真实值暴露为脚本常量。 |
| `#[godot_enum]` / `#[godot_flags]` | 定义可向前兼容的项目枚举和位标志。 |

impl 上的 `#[script]` 是稳定版 Rust 中收集独立 impl 块所需的编译期标记。纯 Rust
辅助方法可以放在不带标记的普通 impl 中。同一类型的多个 `#[script] impl` 会
合并为一张确定性的分发表。

`godot_rs::prelude::*` 是面向脚本的一行便捷导入，仍然可以改用普通 Rust 的
显式导入。

## 高级用法

### 属性、访问器、枚举和位标志

`#[property]` 与 `#[export]` 使用相同的类型化存储和访问器通道，区别仅在于是否
显示到 Inspector。`read_only` 会同时在 Godot 元数据和项目 ABI 层强制执行。

```rust
#[godot_enum]
enum MovementMode {
    Idle = 0,
    #[default]
    Walk = 1,
    Sprint = 2,
}

#[godot_flags]
enum Ability {
    Jump = 1 << 0,
    Dash = 1 << 1,
}

#[script(base = CharacterBody2D, class_name = Player)]
pub struct Player {
    stamina_cache: f64,

    #[property(default = 100, get = health, set = set_health)]
    health: i64,

    #[property(default = 1, read_only)]
    level: i64,

    #[export(default = MovementMode::Walk)]
    movement: MovementMode,

    #[export(default = Ability::Jump | Ability::Dash)]
    abilities: Ability,

    #[node("%Sprite")]
    sprite: NodeRef<AnimatedSprite2D>,
}

#[script]
impl Player {
    pub const MAX_HEALTH: i64 = 100;

    fn health(&self) -> i64 {
        self.health
    }

    fn set_health(&mut self, value: i64) {
        self.health = value.clamp(0, Self::MAX_HEALTH);
    }

    #[rpc(name = "sync_health", authority, reliable)]
    fn apply_network_health(&mut self, value: i64) {
        self.set_health(value);
    }
}
```

自定义枚举和位标志会保留未知整数值，因此读取由更新版本游戏写入的数据时，不会
构造出非法的 Rust 枚举。

### 继承、Resource 和全局类

- 只有确实需要全局注册脚本类时才添加 `class_name = Name`。
- 使用 `base = Resource` 创建 Rust 自定义 Resource 脚本。
- 使用 `extends = "res://path/to/base.rs"` 继承项目内的 Rust 脚本，再通过类型化
  `self.call_super(...)` 调用继承链中的下一层实现。
- 循环继承、父脚本缺失、基类不一致、成员重复和无效反射签名都会在模块生效前
  被拒绝。

### 异步任务与引擎 API

- `spawn`、`next_frame`、`sleep`、`timeout` 和类型化 Signal Future 在 Godot
  主线程协作运行；拥有数据的阻塞任务可以使用 `spawn_blocking`。
- `self.base()` 表示当前 Godot 对象。生成的类方法、构造函数、Builtin、Utility、
  单例、常量、信号和虚接口均使用项目选择的官方 API。
- `godot_debug!`、`godot_print!`、`godot_warn!` 和 `godot_error!` 在两种项目模式
  中都会进入 Godot 调试输出。
- 脚本错误通过 `EngineResult`/`ScriptResult` 返回；Rust Panic 不会展开到 Godot。

### 热重载状态策略

- Inspector 中的 `#[export]` 值默认保留。
- `#[reload(persist)]` 明确保留受支持的非导出字段。
- `#[reload(skip)]` 和普通内部字段使用新构建中的默认值。
- Node 字段会根据当前场景重新解析。
- 字段、方法、继承关系或 ABI 不兼容时保留上一代，并在需要时提示重新运行场景
  或重启编辑器。

## 项目模式

| 模式 | 适用场景 | 运行方式 |
| --- | --- | --- |
| `script` | 推荐用于常规 Godot 游戏脚本。 | `.rs` Script 资源通过插件 Host 运行，支持脚本继承和事务式状态迁移。 |
| `extension` | 用于直接 ClassDB 集成和底层引擎扩展。 | Rust `NativeClass` 通过 Godot 原生扩展接口直接注册，并在受支持的重载中恢复已注册属性。 |

在 `Cargo.toml` 中选择模式：

```toml
[package.metadata.godot-rust]
enabled = true
mode = "script"
godot = "4.4"
scripts = "src/scripts"
```

`godot` 为整个 Rust 构建选择官方 Major/Minor API，可填写 `4.4`、`4.5`、`4.6`
或 `4.7`，不能高于正在运行的编辑器或游戏版本。配置有意不接受 Patch 版本。

## 工作流配置

```toml
[package.metadata.godot-rust]
auto-check = true
build-before-play = true
auto-build-on-idle = false
```

| 配置项 | 默认行为 |
| --- | --- |
| `auto-check` | 合并短时间内的连续保存，并在后台执行 Cargo Check。 |
| `build-before-play` | F5/F6 启动游戏前必须存在经过验证的当前构建。 |
| `auto-build-on-idle` | 当 `auto-check = false` 时，在项目发生变化且编辑器空闲后自动构建。 |

Cargo Workspace 可以通过 `[workspace.metadata.godot-rust].package` 选择所属包。
插件还提供依赖编辑、通过 rustup 安装精确 Rust Target、安全模式、支持信息包和
外部编辑器集成，并且不会覆盖用户已有文件。

## 导出支持

直接使用 Godot 自带的 **项目 > 导出** 窗口。插件会根据 Godot 预设选择 Cargo
Debug/Release Profile，构建启用的架构，逐个校验输出，并且只把通过验证的产物
交给 Godot 导出器。

| 平台 | 支持的 Rust 目标 |
| --- | --- |
| Linux | x86_32、x86_64、ARM32、ARM64 |
| Windows | x86_32、x86_64、ARM64 |
| macOS | x86_64、ARM64、Universal |
| Android | ARMv7、ARM64、x86、x86_64 |
| iOS | ARM64 真机，以及 ARM64/x86_64 Simulator XCFramework 切片 |
| Web | `wasm32-unknown-emscripten` 和匹配的 Godot 动态扩展模板 |

桌面 Rust 导出必须在对应操作系统上执行；Apple 平台需要 macOS 和 Xcode；
Android 与 Web 还需要各自的平台工具链。缺少工具、Target、模板、签名或 ABI
校验失败时会直接阻止导出，不会生成不完整的包。

## 开发本仓库

仓库中的 `example/` 是一个完整的 Godot 项目，其中的 `addons/godot-rust` 也是
流水线组装发布插件时使用的唯一插件源码目录。

```sh
git clone https://github.com/godothub/godot-rust.git
cd godot-rust
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
python3 tools/stage_example_plugin.py
```

仓库工具统一使用 Python 3.11 或更高版本；API 校验和发布工具需要其标准库中的
`tomllib`。装配命令会把当前平台的编辑器 Host、构建服务和模块校验器构建到
示例项目。装配完成后，用 Godot 4.4 或更高版本打开 `example/project.godot`。

### 仓库结构

| 路径 | 职责 |
| --- | --- |
| `example/` | 可运行的 Godot + Cargo 示例项目和唯一插件源码目录。 |
| `crates/godot_rs/` | 用户项目导入的公开 Rust SDK。 |
| `crates/godot_macro/` | 由 `godot_rs` 重新导出的过程宏。 |
| `crates/godot_api/` | 分版本官方 API 模型、生成绑定和稳定项目 ABI。 |
| `crates/godot_host/` | Godot ScriptLanguage Host 和运行时集成。 |
| `crates/godot_build/` | Cargo 配置、诊断、构建、热重载发布和导出编排。 |
| `crates/godot_codegen/` | 官方 API 包生成和完整覆盖策略。 |
| `tools/` | Release、集成测试、平台导出和 API 校验工具。 |

用户项目只需要直接依赖 `godot_rs`。宏和 API 包是它的实现依赖，所需内容已经
由 `godot_rs` 重新导出。

## 文档

- [示例项目与完整工作流说明](example/README.md)
- [`godot_rs` SDK 简介](crates/godot_rs/README.md)
- [English changelog](CHANGELOG.md)
- [中文更新日志](CHANGELOG-ZH.md)

## 许可证

本项目使用 [MIT License](LICENSE)。发布插件还会包含生成 Rust 接口时所使用的
Godot 官方 API 输入对应的许可证信息。
