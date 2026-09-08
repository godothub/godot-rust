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

例如，给 `Node2D` 挂载下面的脚本，用方向键控制移动：

```rust
use godot_rs::prelude::*;

#[script(base = Node2D)]
pub struct Player;

#[script]
impl Player {
    fn _process(&mut self, delta: f64) -> EngineResult<()> {
        let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
        self.base().translate(direction * (240.0 * delta) as f32)?;
        Ok(())
    }
}
```

`#[script]` 声明脚本及其回调，`self.base()` 用于操作挂载脚本的节点。
声明 `_process` 后会自动启用逐帧回调。

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

## 高级用法

下面将 `Player` 扩展为含字段的结构体：把 `pub struct Player;` 改为
`pub struct Player { ... }`，并将字段放在花括号内。方法放进对应的
`#[script] impl`。带 `?` 的操作语句可放在返回 `EngineResult` 的回调中。

### 输入与默认参数

单例可以直接调用，常用参数沿用 Godot 的默认值：

```rust
let direction = Input::get_vector("ui_left", "ui_right", "ui_up", "ui_down")?;
let pressed = Input::is_action_pressed("ui_accept")?;
```

需要修改可选参数时，使用 `*_ex(...)`，最后调用 `.done()`：

```rust
let input = Input::singleton()?;
let pressed = input.is_action_pressed_ex("ui_accept")
    .exact_match(true)
    .done()?;
```

### 节点绑定与脚本协作

把常用节点声明为 `Player` 的字段，之后直接使用。`%Health` 对应场景唯一名称，
也可以填写 `"Children/Health"` 这样的节点路径：

```rust
#[node("%Health")]
health: NodeRef<Health>,

#[node("%Camera")]
camera: Option<NodeRef<Camera2D>>,
```

`Health` 可以是另一个 Rust 脚本的类型。给方法加上 `#[func]`，即可从其他脚本调用：

```rust
// health.rs
use godot_rs::prelude::*;

#[script(base = Node)]
pub struct Health {
    #[export(default = 100)]
    points: i64,
}

#[script]
impl Health {
    #[func]
    fn damage(&mut self, amount: i64) -> i64 {
        self.points = (self.points - amount).max(0);
        self.points
    }
}
```

在 `player.rs` 中添加 `use super::health::Health;`，给场景中的 Health 节点挂载
`health.rs` 并设置唯一名称。模块路径对应脚本文件：同目录使用
`super::health::Health`，嵌套目录则可以使用
`crate::scripts::actors::enemy::Enemy`。然后在 `Player` 的回调中调用：

```rust
let remaining = self.health.damage(10)?;
let current = self.health.properties().points()?;
self.health.properties().set_points(100)?;
```

方法参数和返回值有编译检查，属性访问遵循声明的 getter/setter。
必需节点缺失或挂载了错误的脚本会报告错误；可选节点不存在时为 `None`。
临时查找节点可用 `self.base().node::<Health>("%Health")?`。
若脚本方法与引擎方法同名，用 `self.health.script().method(...)` 明确调用脚本方法。

### Inspector 属性与私有状态

用 `#[export]` 配置 Inspector，用 `#[property]` 提供不显示在 Inspector 的属性。
其他字段保持为私有 Rust 状态，可以直接用表达式初始化。
例如，给 `Player` 添加以下字段：

```rust
#[export(default = 240.0, group = "Movement", range(min = 0, max = 1000))]
speed: f64,

#[property(default = 1, read_only)]
level: i64,

#[init(default = Vec::with_capacity(32))]
recent_hits: Vec<i64>,
```

要让 `speed` 控制前面的移动，将 `_process` 中的 `240.0` 改为 `self.speed`。
初始化表达式用于创建脚本状态，适合准备缓存、容器等私有数据。
Rust 内部的字段赋值遵循普通 Rust 语义；需要校验或发送通知时，显式调用自己的 setter。

### 自定义资源

把配置独立为 Resource，可以在多个角色、武器或关卡之间复用：

```rust
use godot_rs::prelude::*;

#[script(base = Resource)]
pub struct WeaponConfig {
    #[export(default = 25)]
    damage: i64,
}
```

将脚本保存为 `weapon_config.rs`，挂到 Resource 并保存为 `.tres`。
在使用它的脚本中添加 `use super::weapon_config::WeaponConfig;`，然后加载资源：

```rust
let weapon = load::<WeaponConfig>("res://weapons/sword.tres")?;
let damage = weapon.properties().damage()?;
```

也可以声明 `#[export] weapon: Option<GodotRef<WeaponConfig>>`，从 Inspector 指定资源。
`GodotRef<T>` 会保持资源存活；`NodeRef<T>` 则引用由 Godot 管理的节点，不会阻止节点释放。

### 信号与回调

给上面的 `Health` 结构体添加信号字段，参数名称与类型写在一起：

```rust
#[signal]
damaged: Signal<fn(amount: i64, remaining: i64)>,
```

将 `Health::damage` 替换为下面的方法，让每次伤害都发送信号：

```rust
#[func]
fn damage(&mut self, amount: i64) -> EngineResult<i64> {
    self.points = (self.points - amount).max(0);
    self.damaged.emit(amount, self.points)?;
    Ok(self.points)
}
```

给 `Player` 添加 `_ready` 回调，在其他游戏逻辑调用 `damage` 前连接信号：

```rust
fn _ready(&mut self) -> EngineResult<()> {
    self.health.signals().damaged()
        .connect_deferred(&self.callbacks()?.on_damaged())?;
    Ok(())
}
```

将目标回调添加到 `Player` 的 `#[script] impl` 中：

```rust
#[func]
fn on_damaged(&mut self, amount: i64, remaining: i64) {
    godot_print!("受到 {amount} 点伤害，剩余 {remaining}");
}
```

参数不匹配会在编译时报错。`connect_method(...)` 同步接收，
`connect_deferred(...)` 延迟接收，适合信号会回到当前正在执行的脚本的情况。
仍然可以使用 Godot 的信号面板连接。需要自动断开时，使用 `connect_scoped(...)`
并保存返回的连接对象，丢弃它即可断开。

### 等待信号与异步任务

沿用上面的 `Health::damaged` 信号，在 `Player` 的回调中创建等待，
再交给脚本自己的任务执行：

```rust
let damaged = self.health.signals().damaged().wait();
self.tasks().spawn(async move {
    match damaged.await {
        Ok((amount, remaining)) => godot_print!("伤害 {amount}，剩余 {remaining}"),
        Err(error) => godot_warn!("等待中断：{error}"),
    }
});
```

等待从任务首次执行时开始监听。`self.tasks()` 中的任务会在脚本销毁或热重载替换时取消；
若要在节点退出场景树时停止任务，可保存返回的 `TaskHandle` 并在 `_exit_tree` 中取消。
还可以使用 `next_frame()`、`sleep(...)` 和 `timeout(...)`，无需阻塞游戏主线程。
耗时的纯计算可交给 `spawn_blocking(...)`，节点和资源操作保留在主线程。
