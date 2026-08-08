@tool
extends RefCounted

const LIVE_OFF := 0
const LIVE_POLITE := 1

const ZH_CN := {
	"Rust": "Rust",
	"Rust status": "Rust 状态",
	"Rust diagnostics": "Rust 诊断",
	"Rust diagnostics table": "Rust 诊断表格",
	"Rust tools are ready.": "Rust 工具已就绪。",
	"Check": "检查",
	"Build": "构建",
	"Cancel": "取消",
	"Preview Fix": "预览修复",
	"Undo Fix": "撤销修复",
	"Clear": "清除",
	"Support Bundle": "支持包",
	"Repair Toolchain": "修复工具链",
	"Run Cargo Check for the configured Rust package.": "对已配置的 Rust 包运行 Cargo Check。",
	"Build and validate the configured Rust package.": "构建并验证已配置的 Rust 包。",
	"Cancel the active Rust operation.": "取消当前 Rust 操作。",
	"Preview and apply a machine-applicable suggestion from rustc.": "预览并应用 rustc 提供的可自动执行建议。",
	"Restore files changed by the last Rust quick fix.": "恢复上一次 Rust 快速修复修改的文件。",
	"Clear Rust diagnostics from this panel.": "清除此面板中的 Rust 诊断。",
	"Create a redacted ZIP with tool versions and bounded diagnostics. Project source, environment variables, and absolute paths are excluded.": "创建包含工具版本和有限诊断信息的脱敏 ZIP；不会包含项目源码、环境变量值或绝对路径。",
	"Install a supported Rust standard-library target through rustup. Platform SDKs and linkers are never modified automatically.": "通过 rustup 安装受支持的 Rust 标准库目标；不会自动修改平台 SDK 或链接器。",
	"Level": "级别",
	"Code": "代码",
	"Message": "消息",
	"Location": "位置",
	"Error": "错误",
	"Warning": "警告",
	"Failure": "失败",
	"Note": "说明",
	"Help": "帮助",
	"Info": "信息",
	"quick fix available": "可使用快速修复",
	"Preview Rust Quick Fix": "预览 Rust 快速修复",
	"Apply Fix": "应用修复",
	"Replace with: %s": "替换为：%s",
	"%s is running…": "%s正在运行……",
	"%s failed: %s": "%s失败：%s",
	"%s succeeded": "%s成功",
	"%s failed": "%s失败",
	"%d error(s) · %d warning(s)": "%d 个错误 · %d 个警告",
	"showing first %d of %d diagnostics": "显示前 %d 条，共 %d 条诊断",
	"Rust diagnostics cleared.": "Rust 诊断已清除。",
	"Rust quick fix": "Rust 快速修复",
	"Cargo Dependencies": "Cargo 依赖",
	"Cargo dependency manager": "Cargo 依赖管理器",
	"Close": "关闭",
	"Load the configured Cargo package dependencies.": "加载已配置 Cargo 包的依赖。",
	"Reload": "重新加载",
	"Reload dependencies from Cargo.toml.": "从 Cargo.toml 重新加载依赖。",
	"Cargo dependency entries": "Cargo 依赖项",
	"%s · %d direct dependency entries": "%s · %d 个直接依赖项",
	"Kind": "类型",
	"Target": "目标",
	"Dependency": "依赖",
	"Version / Source": "版本 / 来源",
	"Features": "Features",
	"Development": "开发",
	"Normal": "普通",
	"Name": "名称",
	"Requirement": "版本要求",
	"Source": "来源",
	"Source value": "来源值",
	"Git reference": "Git 引用",
	"Options": "选项",
	"Registry": "注册表",
	"Git": "Git",
	"Path": "路径",
	"None": "无",
	"Branch": "分支",
	"Tag": "标签",
	"Revision": "提交",
	"Default features": "默认 Features",
	"Optional": "可选依赖",
	"Allow source replacement": "允许替换来源",
	"Permit changing between Registry, Git, and Path after preview.": "预览后允许在注册表、Git 和路径来源之间切换。",
	"Preview Add / Update": "预览添加 / 更新",
	"Show the exact Cargo.toml entry before writing.": "写入前显示 Cargo.toml 的准确变更。",
	"Preview Remove": "预览移除",
	"Preview removal of the selected dependency.": "预览移除选中的依赖。",
	"Review Cargo Dependency Change": "检查 Cargo 依赖变更",
	"Apply": "应用",
	"Cargo.toml dependency entry before:\n%s\n\nCargo.toml dependency entry after:\n%s": "Cargo.toml 依赖项变更前：\n%s\n\nCargo.toml 依赖项变更后：\n%s",
	"(dependency is not present)": "（依赖项不存在）",
	"(dependency will be removed)": "（依赖项将被移除）",
	"Dependency operation failed: %s": "依赖操作失败：%s",
	"Select or enter a dependency name first.": "请先选择或输入依赖名称。",
	"Empty, target triple, or cfg(...)": "留空、目标三元组或 cfg(...)",
	"Leave empty for all targets, or enter a Cargo target triple or cfg expression.": "留空表示所有目标，或输入 Cargo 目标三元组 / cfg 表达式。",
	"Registry alias, Git URL, or dependency path": "注册表别名、Git URL 或依赖路径",
	"Optional Cargo registry alias": "可选的 Cargo 注册表别名",
	"Cargo package used for Rust Check, Build, Run, and Export.": "用于 Rust 检查、构建、运行和导出的 Cargo 包。",
	"Initialize Rust for this Godot project?": "为此 Godot 项目初始化 Rust？",
	"Initialize": "初始化",
	(
		"godot-rust did not find a Cargo.toml.\n\n"
		+ "It can initialize a standard Cargo library in the project root "
		+ "with:\n%s\n\nIt will then add the godot_rs dependency, Script "
		+ "Mode metadata, dynamic-library targets, and generated script "
		+ "entry files. Existing Cargo files and src/lib.rs are never "
		+ "overwritten."
	): (
		"godot-rust 没有找到 Cargo.toml。\n\n"
		+ "可以在项目根目录用以下命令初始化标准 Cargo 库：\n%s\n\n"
		+ "随后会添加 godot_rs 依赖、Script Mode 元数据、动态库目标和生成的"
		+ "脚本入口文件。现有 Cargo 文件和 src/lib.rs 绝不会被覆盖。"
	),
	"Configure this Cargo package for godot-rust?": "为此 Cargo 包配置 godot-rust？",
	"Configure": "配置",
	(
		"godot-rust found a Cargo package that is not ready for Rust scripts.\n\n"
		+ "It can add missing Script Mode settings, the compatible godot_rs "
		+ "dependency, cdylib/rlib targets, `mod scripts;`, and the Cargo cache "
		+ "directory. Existing values, dependencies, source code, comments, "
		+ "and formatting are preserved. "
		+ "Any conflict stops setup before files change."
	): (
		"godot-rust 找到了尚未配置 Rust 脚本的 Cargo 包。\n\n"
		+ "可以补充 Script Mode 设置、兼容的 godot_rs 依赖、cdylib/rlib "
		+ "目标、`mod scripts;` 和 Cargo 缓存目录。现有配置、依赖、源码、"
		+ "注释和格式都会保留；出现冲突时会在修改文件前停止。"
	),
	"Select the Rust Cargo package": "选择 Rust Cargo 包",
	"Use Package": "使用此包",
	(
		"This Cargo workspace has multiple packages that can provide the "
		+ "Godot Rust scripts. Select the package owned by this Godot project."
		+ "\n\ngodot-rust will persist the choice in "
		+ "[workspace.metadata.godot-rust].package."
	): (
		"此 Cargo 工作区中有多个可以提供 Godot Rust 脚本的包。请选择属于"
		+ "当前 Godot 项目的包。\n\ngodot-rust 会把选择保存到 "
		+ "[workspace.metadata.godot-rust].package。"
	),
	"Install a Rust export target?": "安装 Rust 导出目标？",
	"Install Target": "安装目标",
	(
		"Select a supported Rust standard-library target to install with "
		+ "`rustup target add`.\n\nThis action never installs or changes Xcode, "
		+ "Android SDK/NDK, Emscripten, system compilers, or linkers."
	): (
		"选择一个受支持的 Rust 标准库目标，并使用 `rustup target add` "
		+ "安装。\n\n此操作绝不会安装或更改 Xcode、Android SDK/NDK、"
		+ "Emscripten、系统编译器或链接器。"
	),
	"Exact rustup target used by the selected Godot export platform.": "所选 Godot 导出平台使用的准确 rustup 目标。",
	"Rust: Project Status": "Rust：项目状态",
	"Rust: Check": "Rust：检查",
	"Rust: Build": "Rust：构建",
	"Rust: Dependencies": "Rust：依赖",
	"Rust: Toggle Safe Mode": "Rust：切换安全模式",
	"Rust: Create Support Bundle": "Rust：创建支持包",
	"Rust: Repair Toolchain...": "Rust：修复工具链……",
	"project probe": "项目检测",
	"Rust check": "Rust 检查",
	"Rust build": "Rust 构建",
	"Automatic Rust check": "自动 Rust 检查",
	"Automatic Rust build": "自动 Rust 构建",
	"Rust build before play": "运行前 Rust 构建",
	"Rust support bundle": "Rust 支持包",
	"Rust toolchain repair": "Rust 工具链修复",
	"Cargo dependencies": "Cargo 依赖",
	"Cargo dependency preview": "Cargo 依赖预览",
	"Cargo dependency apply": "Cargo 依赖应用",
	"Undo Rust quick fix": "撤销 Rust 快速修复",
}

static var _locale_override := ""


static func text(message: String) -> String:
	var locale := (
		_locale_override
		if not _locale_override.is_empty()
		else TranslationServer.get_tool_locale()
	)
	if locale.to_lower().begins_with("zh"):
		return str(ZH_CN.get(message, message))
	return message


static func set_locale_override(locale: String) -> void:
	_locale_override = locale


static func scaled_size(size: Vector2i, scale: float) -> Vector2i:
	return Vector2i(
		roundi(float(size.x) * maxf(scale, 1.0)),
		roundi(float(size.y) * maxf(scale, 1.0))
	)


static func configure_control(
	control: Control,
	accessible_name: String,
	description := "",
	live := false,
	focusable := true
) -> void:
	if focusable:
		control.focus_mode = Control.FOCUS_ALL
	if not description.is_empty():
		control.tooltip_text = text(description)
	var version := Engine.get_version_info()
	if (
		int(version.get("major", 0)) < 4
		or (
			int(version.get("major", 0)) == 4
			and int(version.get("minor", 0)) < 5
		)
	):
		return
	control.set("accessibility_name", text(accessible_name))
	if not description.is_empty():
		control.set("accessibility_description", text(description))
	control.set("accessibility_live", LIVE_POLITE if live else LIVE_OFF)


static func button_shortcut(
	keycode: int,
	shift := true,
	alt := true
) -> Shortcut:
	var event := InputEventKey.new()
	event.physical_keycode = keycode
	event.shift_pressed = shift
	event.alt_pressed = alt
	var value := Shortcut.new()
	value.events = [event]
	return value
