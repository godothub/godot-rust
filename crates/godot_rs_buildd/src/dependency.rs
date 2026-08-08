use crate::managed_fs::atomic_write;
use crate::{CargoPackageModel, inspect_cargo_project};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DEPENDENCIES: usize = 4_096;
const MAX_FEATURES: usize = 256;
const MAX_FIELD_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyKind {
    Normal,
    Development,
    Build,
}

impl DependencyKind {
    const fn table_name(self) -> &'static str {
        match self {
            Self::Normal => "dependencies",
            Self::Development => "dev-dependencies",
            Self::Build => "build-dependencies",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyOperation {
    Upsert,
    Remove,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencySourceKind {
    Registry,
    Git,
    Path,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DependencyChange {
    pub operation: DependencyOperation,
    pub kind: DependencyKind,
    #[serde(default)]
    pub target: String,
    pub name: String,
    #[serde(default)]
    pub requirement: String,
    pub source_kind: DependencySourceKind,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub git_reference_kind: String,
    #[serde(default)]
    pub git_reference: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default = "default_true")]
    pub default_features: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub allow_source_change: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DependencyRecord {
    pub kind: DependencyKind,
    pub target: Option<String>,
    pub name: String,
    pub package: Option<String>,
    pub requirement: Option<String>,
    pub source_kind: DependencySourceKind,
    pub source: Option<String>,
    pub git_reference_kind: Option<String>,
    pub git_reference: Option<String>,
    pub features: Vec<String>,
    pub default_features: bool,
    pub optional: bool,
    pub inherited: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DependencyListReport {
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub dependencies: Vec<DependencyRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DependencyPreview {
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub expected_sha256: String,
    pub change: DependencyChange,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DependencyApplyReport {
    pub package_name: String,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub changed: bool,
}

struct ManifestEdit {
    path: PathBuf,
    original: String,
    replacement: String,
}

pub fn list_dependencies(
    project_root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<DependencyListReport, String> {
    let project = inspect_cargo_project(project_root, cargo)?;
    let package = selected_package(&project)?;
    let (source, document) = read_manifest(&package.manifest_path)?;
    let mut dependencies = Vec::new();
    for kind in [
        DependencyKind::Normal,
        DependencyKind::Development,
        DependencyKind::Build,
    ] {
        collect_dependencies_from_table(&document, kind, None, &mut dependencies)?;
    }
    if let Some(targets) = document.get("target") {
        let targets = targets
            .as_table_like()
            .ok_or_else(|| "[target] must be a Cargo table".to_owned())?;
        for (target, configuration) in targets.iter() {
            let configuration = configuration
                .as_table_like()
                .ok_or_else(|| format!("Cargo target configuration `{target}` must be a table"))?;
            for kind in [
                DependencyKind::Normal,
                DependencyKind::Development,
                DependencyKind::Build,
            ] {
                collect_dependencies_from_table_like(
                    configuration,
                    kind,
                    Some(target),
                    &mut dependencies,
                )?;
            }
        }
    }
    if dependencies.iter().any(|dependency| dependency.inherited) {
        let workspace_path = project.workspace_root.join("Cargo.toml");
        let workspace_document = if workspace_path == package.manifest_path {
            document.clone()
        } else {
            read_manifest(&workspace_path)?.1
        };
        for dependency in dependencies
            .iter_mut()
            .filter(|dependency| dependency.inherited)
        {
            enrich_inherited_dependency(dependency, &workspace_document)?;
        }
    }
    dependencies.sort_by(|left, right| {
        (left.target.as_deref(), left.kind as u8, left.name.as_str()).cmp(&(
            right.target.as_deref(),
            right.kind as u8,
            right.name.as_str(),
        ))
    });
    Ok(DependencyListReport {
        package_name: package.name,
        manifest_path: package.manifest_path,
        manifest_sha256: sha256(source.as_bytes()),
        dependencies,
    })
}

pub fn preview_dependency_change(
    project_root: impl AsRef<Path>,
    change: DependencyChange,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<DependencyPreview, String> {
    let cargo = cargo.map(|value| value.as_ref().to_owned());
    plan_dependency_change(project_root.as_ref(), change, cargo.as_deref())
        .map(|(preview, _)| preview)
}

pub fn apply_dependency_change(
    project_root: impl AsRef<Path>,
    expected_sha256: &str,
    change: DependencyChange,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<DependencyApplyReport, String> {
    validate_hash(expected_sha256)?;
    let cargo = cargo.map(|value| value.as_ref().to_owned());
    let (preview, edits) = plan_dependency_change(project_root.as_ref(), change, cargo.as_deref())?;
    if preview.expected_sha256 != expected_sha256 {
        return Err(
            "Cargo manifest changed after the dependency preview; reload and review it again"
                .to_owned(),
        );
    }
    apply_manifest_edits(&edits)?;
    let changed = !edits.is_empty();
    let (_, final_document) = read_manifest(&preview.manifest_path)?;
    let final_source = final_document.to_string();
    Ok(DependencyApplyReport {
        package_name: preview.package_name,
        manifest_path: preview.manifest_path,
        manifest_sha256: sha256(final_source.as_bytes()),
        changed,
    })
}

fn plan_dependency_change(
    project_root: &Path,
    change: DependencyChange,
    cargo: Option<&OsStr>,
) -> Result<(DependencyPreview, Vec<ManifestEdit>), String> {
    validate_change(&change)?;
    let project = inspect_cargo_project(project_root, cargo)?;
    let package = selected_package(&project)?;
    let (member_source, mut member_document) = read_manifest(&package.manifest_path)?;
    let existing = dependency_table_mut(&mut member_document, change.kind, &change.target)?
        .get(&change.name)
        .cloned();
    let inherited = existing.as_ref().is_some_and(dependency_is_inherited);
    let member_before = existing.as_ref().map(Item::to_string).unwrap_or_default();

    if change.operation == DependencyOperation::Remove {
        if dependency_table_mut(&mut member_document, change.kind, &change.target)?
            .remove(&change.name)
            .is_none()
        {
            let location = dependency_location_label(change.kind, &change.target);
            return Err(format!(
                "Dependency `{}` does not exist in {location}",
                change.name,
            ));
        }
    } else if inherited {
        dependency_table_mut(&mut member_document, change.kind, &change.target)?
            .insert(&change.name, render_workspace_member(&change));
    } else {
        validate_source_change(existing.as_ref(), &change)?;
        dependency_table_mut(&mut member_document, change.kind, &change.target)?
            .insert(&change.name, render_dependency(&change, existing.as_ref())?);
    }
    let member_after = dependency_table_mut(&mut member_document, change.kind, &change.target)?
        .get(&change.name)
        .map(Item::to_string)
        .unwrap_or_default();

    let workspace_path = project.workspace_root.join("Cargo.toml");
    let mut before = member_before;
    let mut after = member_after;
    let mut originals = vec![member_source.clone()];
    let mut edits = Vec::new();
    if inherited && change.operation == DependencyOperation::Upsert {
        if workspace_path == package.manifest_path {
            let workspace = workspace_dependency_table_mut(&mut member_document)?;
            let workspace_existing = workspace.get(&change.name).cloned().ok_or_else(|| {
                format!(
                    "Workspace dependency `{}` is missing from [workspace.dependencies]",
                    change.name
                )
            })?;
            validate_source_change(Some(&workspace_existing), &change)?;
            let workspace_before = workspace_existing.to_string();
            let workspace_change = workspace_source_change(&change, &workspace_existing);
            workspace.insert(
                &change.name,
                render_dependency(&workspace_change, Some(&workspace_existing))?,
            );
            let workspace_after = workspace
                .get(&change.name)
                .map(Item::to_string)
                .unwrap_or_default();
            before = format!("Package entry: {before}\nWorkspace entry: {workspace_before}");
            after = format!("Package entry: {after}\nWorkspace entry: {workspace_after}");
        } else {
            let (workspace_source, mut workspace_document) = read_manifest(&workspace_path)?;
            let workspace = workspace_dependency_table_mut(&mut workspace_document)?;
            let workspace_existing = workspace.get(&change.name).cloned().ok_or_else(|| {
                format!(
                    "Workspace dependency `{}` is missing from [workspace.dependencies]",
                    change.name
                )
            })?;
            validate_source_change(Some(&workspace_existing), &change)?;
            let workspace_before = workspace_existing.to_string();
            let workspace_change = workspace_source_change(&change, &workspace_existing);
            workspace.insert(
                &change.name,
                render_dependency(&workspace_change, Some(&workspace_existing))?,
            );
            let workspace_after = workspace
                .get(&change.name)
                .map(Item::to_string)
                .unwrap_or_default();
            before = format!("Package entry: {before}\nWorkspace entry: {workspace_before}");
            after = format!("Package entry: {after}\nWorkspace entry: {workspace_after}");
            originals.push(workspace_source.clone());
            let workspace_replacement = workspace_document.to_string();
            if workspace_replacement != workspace_source {
                edits.push(ManifestEdit {
                    path: workspace_path,
                    original: workspace_source,
                    replacement: workspace_replacement,
                });
            }
        }
    }
    let member_replacement = member_document.to_string();
    if member_replacement != member_source {
        edits.insert(
            0,
            ManifestEdit {
                path: package.manifest_path.clone(),
                original: member_source,
                replacement: member_replacement,
            },
        );
    }
    let expected_sha256 = combined_manifest_hash(&originals);
    Ok((
        DependencyPreview {
            package_name: package.name,
            manifest_path: package.manifest_path,
            expected_sha256,
            change,
            before,
            after,
        },
        edits,
    ))
}

fn dependency_table_mut<'a>(
    document: &'a mut DocumentMut,
    kind: DependencyKind,
    target: &str,
) -> Result<&'a mut dyn toml_edit::TableLike, String> {
    let table_name = kind.table_name();
    if target.is_empty() {
        if document.get(table_name).is_none() {
            document[table_name] = Item::Table(toml_edit::Table::new());
        }
        return document[table_name]
            .as_table_like_mut()
            .ok_or_else(|| format!("[{table_name}] must be a Cargo dependency table"));
    }

    if document.get("target").is_none() {
        document["target"] = Item::Table(toml_edit::Table::new());
    }
    let targets = document["target"]
        .as_table_like_mut()
        .ok_or_else(|| "[target] must be a Cargo table".to_owned())?;
    if targets.get(target).is_none() {
        targets.insert(target, Item::Table(toml_edit::Table::new()));
    }
    let configuration = targets
        .get_mut(target)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| format!("Cargo target configuration `{target}` must be a table"))?;
    if configuration.get(table_name).is_none() {
        configuration.insert(table_name, Item::Table(toml_edit::Table::new()));
    }
    configuration
        .get_mut(table_name)
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| {
            format!(
                "{} must be a Cargo dependency table",
                dependency_location_label(kind, target)
            )
        })
}

fn workspace_dependency_table_mut(
    document: &mut DocumentMut,
) -> Result<&mut dyn toml_edit::TableLike, String> {
    document
        .get_mut("workspace")
        .and_then(Item::as_table_like_mut)
        .and_then(|workspace| workspace.get_mut("dependencies"))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| "Cargo workspace has no [workspace.dependencies] table".to_owned())
}

fn render_workspace_member(change: &DependencyChange) -> Item {
    let mut table = InlineTable::new();
    table.insert("workspace", Value::from(true));
    if !change.features.is_empty() {
        let mut features = Array::new();
        for feature in &change.features {
            features.push(feature);
        }
        table.insert("features", Value::Array(features));
    }
    if !change.default_features {
        table.insert("default-features", Value::from(false));
    }
    if change.optional {
        table.insert("optional", Value::from(true));
    }
    Item::Value(Value::InlineTable(table))
}

fn workspace_source_change(change: &DependencyChange, existing: &Item) -> DependencyChange {
    let mut workspace = change.clone();
    workspace.target.clear();
    if let Some(table) = existing.as_table_like() {
        workspace.features = table
            .get("features")
            .and_then(Item::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        workspace.default_features = table_bool(table, "default-features").unwrap_or(true);
        workspace.optional = table_bool(table, "optional").unwrap_or(false);
    } else {
        workspace.features.clear();
        workspace.default_features = true;
        workspace.optional = false;
    }
    workspace
}

fn validate_source_change(
    existing: Option<&Item>,
    change: &DependencyChange,
) -> Result<(), String> {
    if let Some(existing) = existing {
        let existing_source = dependency_source_kind(existing);
        if existing_source != change.source_kind && !change.allow_source_change {
            return Err(format!(
                "Dependency `{}` currently uses {} source, but the requested change uses {}. \
                 Enable source replacement only after reviewing the preview",
                change.name,
                source_label(existing_source),
                source_label(change.source_kind)
            ));
        }
    }
    Ok(())
}

fn apply_manifest_edits(edits: &[ManifestEdit]) -> Result<(), String> {
    for edit in edits {
        let (current, _) = read_manifest(&edit.path)?;
        if current != edit.original {
            return Err(format!(
                "Cargo manifest changed before the dependency update could be written: {}",
                edit.path.display()
            ));
        }
    }
    for (applied, edit) in edits.iter().enumerate() {
        if let Err(error) = atomic_write(
            &edit.path,
            edit.replacement.as_bytes(),
            "Cargo dependency manifest",
        ) {
            let mut rollback_errors = Vec::new();
            for previous in edits[..applied].iter().rev() {
                if let Err(error) = atomic_write(
                    &previous.path,
                    previous.original.as_bytes(),
                    "Cargo dependency rollback",
                ) {
                    rollback_errors.push(error);
                }
            }
            if rollback_errors.is_empty() {
                return Err(error);
            }
            return Err(format!(
                "{error}; dependency rollback also failed: {}",
                rollback_errors.join("; ")
            ));
        }
    }
    Ok(())
}

fn combined_manifest_hash(sources: &[String]) -> String {
    let mut hasher = Sha256::new();
    for source in sources {
        hasher.update((source.len() as u64).to_le_bytes());
        hasher.update(source.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn selected_package(project: &crate::CargoProjectModel) -> Result<CargoPackageModel, String> {
    project
        .selected_package
        .clone()
        .or_else(|| project.native_package.clone())
        .ok_or_else(|| {
            if project.issues.is_empty() {
                "Cargo project has no selected godot-rust package".to_owned()
            } else {
                format!(
                    "Cargo project has no selected godot-rust package: {}",
                    project.issues.join("; ")
                )
            }
        })
}

fn read_manifest(path: &Path) -> Result<(String, DocumentMut), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect Cargo manifest `{}`: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Cargo manifest must be a regular non-symlink file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "Cargo manifest exceeds the {MAX_MANIFEST_BYTES} byte limit: {}",
            path.display()
        ));
    }
    let source = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read Cargo manifest `{}`: {error}",
            path.display()
        )
    })?;
    let document = source.parse::<DocumentMut>().map_err(|error| {
        format!(
            "Cargo manifest is invalid and was not modified `{}`: {error}",
            path.display()
        )
    })?;
    Ok((source, document))
}

fn collect_dependencies_from_table(
    document: &DocumentMut,
    kind: DependencyKind,
    target: Option<&str>,
    dependencies: &mut Vec<DependencyRecord>,
) -> Result<(), String> {
    let Some(table) = document
        .get(kind.table_name())
        .and_then(Item::as_table_like)
    else {
        return Ok(());
    };
    collect_dependency_entries(table, kind, target, dependencies)
}

fn collect_dependencies_from_table_like(
    configuration: &dyn toml_edit::TableLike,
    kind: DependencyKind,
    target: Option<&str>,
    dependencies: &mut Vec<DependencyRecord>,
) -> Result<(), String> {
    let Some(table) = configuration
        .get(kind.table_name())
        .and_then(Item::as_table_like)
    else {
        return Ok(());
    };
    collect_dependency_entries(table, kind, target, dependencies)
}

fn collect_dependency_entries(
    table: &dyn toml_edit::TableLike,
    kind: DependencyKind,
    target: Option<&str>,
    dependencies: &mut Vec<DependencyRecord>,
) -> Result<(), String> {
    for (name, item) in table.iter() {
        dependencies.push(parse_dependency(kind, target, name, item)?);
        if dependencies.len() > MAX_DEPENDENCIES {
            return Err(format!(
                "Cargo manifest contains more than {MAX_DEPENDENCIES} dependencies"
            ));
        }
    }
    Ok(())
}

fn enrich_inherited_dependency(
    dependency: &mut DependencyRecord,
    workspace_document: &DocumentMut,
) -> Result<(), String> {
    let workspace = workspace_document
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table_like)
        .ok_or_else(|| "Cargo workspace has no [workspace.dependencies] table".to_owned())?;
    let item = workspace.get(&dependency.name).ok_or_else(|| {
        format!(
            "Workspace dependency `{}` is missing from [workspace.dependencies]",
            dependency.name
        )
    })?;
    let shared = parse_dependency(
        dependency.kind,
        dependency.target.as_deref(),
        &dependency.name,
        item,
    )?;
    dependency.package = shared.package;
    dependency.requirement = shared.requirement;
    dependency.source_kind = shared.source_kind;
    dependency.source = shared.source;
    dependency.git_reference_kind = shared.git_reference_kind;
    dependency.git_reference = shared.git_reference;
    Ok(())
}

fn parse_dependency(
    kind: DependencyKind,
    target: Option<&str>,
    name: &str,
    item: &Item,
) -> Result<DependencyRecord, String> {
    if let Some(requirement) = item.as_str() {
        return Ok(DependencyRecord {
            kind,
            target: target.map(str::to_owned),
            name: name.to_owned(),
            package: None,
            requirement: Some(requirement.to_owned()),
            source_kind: DependencySourceKind::Registry,
            source: None,
            git_reference_kind: None,
            git_reference: None,
            features: Vec::new(),
            default_features: true,
            optional: false,
            inherited: false,
        });
    }
    let table = item.as_table_like().ok_or_else(|| {
        format!(
            "Dependency `{name}` in [{}] is not a supported Cargo dependency value",
            kind.table_name()
        )
    })?;
    let source_kind = dependency_source_kind(item);
    let source = match source_kind {
        DependencySourceKind::Registry => table_string(table, "registry"),
        DependencySourceKind::Git => table_string(table, "git"),
        DependencySourceKind::Path => table_string(table, "path"),
    };
    let (git_reference_kind, git_reference) = ["branch", "tag", "rev"]
        .into_iter()
        .find_map(|key| table_string(table, key).map(|value| (key.to_owned(), value)))
        .map_or((None, None), |(kind, value)| (Some(kind), Some(value)));
    Ok(DependencyRecord {
        kind,
        target: target.map(str::to_owned),
        name: name.to_owned(),
        package: table_string(table, "package"),
        requirement: table_string(table, "version"),
        source_kind,
        source,
        git_reference_kind,
        git_reference,
        features: table
            .get("features")
            .and_then(Item::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        default_features: table_bool(table, "default-features").unwrap_or(true),
        optional: table_bool(table, "optional").unwrap_or(false),
        inherited: table_bool(table, "workspace").unwrap_or(false),
    })
}

fn render_dependency(change: &DependencyChange, existing: Option<&Item>) -> Result<Item, String> {
    let package = existing
        .and_then(Item::as_table_like)
        .and_then(|table| table_string(table, "package"));
    if change.source_kind == DependencySourceKind::Registry
        && change.source.is_empty()
        && change.features.is_empty()
        && change.default_features
        && !change.optional
        && package.is_none()
    {
        return Ok(Item::Value(Value::from(change.requirement.clone())));
    }
    let mut table = InlineTable::new();
    if let Some(package) = package {
        table.insert("package", Value::from(package));
    }
    if !change.requirement.is_empty() {
        table.insert("version", Value::from(change.requirement.clone()));
    }
    match change.source_kind {
        DependencySourceKind::Registry => {
            if !change.source.is_empty() {
                table.insert("registry", Value::from(change.source.clone()));
            }
        }
        DependencySourceKind::Git => {
            table.insert("git", Value::from(change.source.clone()));
            if !change.git_reference.is_empty() {
                table.insert(
                    &change.git_reference_kind,
                    Value::from(change.git_reference.clone()),
                );
            }
        }
        DependencySourceKind::Path => {
            table.insert("path", Value::from(change.source.clone()));
        }
    }
    if !change.features.is_empty() {
        let mut features = Array::new();
        for feature in &change.features {
            features.push(feature);
        }
        table.insert("features", Value::Array(features));
    }
    if !change.default_features {
        table.insert("default-features", Value::from(false));
    }
    if change.optional {
        table.insert("optional", Value::from(true));
    }
    Ok(Item::Value(Value::InlineTable(table)))
}

fn validate_change(change: &DependencyChange) -> Result<(), String> {
    validate_field("dependency name", &change.name, false)?;
    validate_field("dependency target", &change.target, true)?;
    if !change
        .name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("Cargo dependency name contains unsupported characters".to_owned());
    }
    if change.operation == DependencyOperation::Remove {
        return Ok(());
    }
    validate_field("version requirement", &change.requirement, true)?;
    validate_field("dependency source", &change.source, true)?;
    if change.source_kind == DependencySourceKind::Registry && change.requirement.is_empty() {
        return Err("Registry dependencies require a version requirement".to_owned());
    }
    if matches!(
        change.source_kind,
        DependencySourceKind::Git | DependencySourceKind::Path
    ) && change.source.is_empty()
    {
        return Err(format!(
            "{} dependencies require a source",
            source_label(change.source_kind)
        ));
    }
    if !change.git_reference.is_empty()
        && !matches!(change.git_reference_kind.as_str(), "branch" | "tag" | "rev")
    {
        return Err("Git reference kind must be branch, tag, or rev".to_owned());
    }
    if change.features.len() > MAX_FEATURES {
        return Err(format!(
            "Dependency declares more than {MAX_FEATURES} features"
        ));
    }
    for feature in &change.features {
        validate_field("dependency feature", feature, false)?;
        if !feature.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.' | b'/' | b'?')
        }) {
            return Err(format!(
                "Dependency feature contains unsupported characters: `{feature}`"
            ));
        }
    }
    Ok(())
}

fn validate_field(label: &str, value: &str, allow_empty: bool) -> Result<(), String> {
    if (!allow_empty && value.is_empty())
        || value.len() > MAX_FIELD_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{label} is empty, too large, or contains control characters"
        ));
    }
    Ok(())
}

fn dependency_is_inherited(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table_bool(table, "workspace"))
        .unwrap_or(false)
}

fn dependency_source_kind(item: &Item) -> DependencySourceKind {
    let Some(table) = item.as_table_like() else {
        return DependencySourceKind::Registry;
    };
    if table.get("git").is_some() {
        DependencySourceKind::Git
    } else if table.get("path").is_some() {
        DependencySourceKind::Path
    } else {
        DependencySourceKind::Registry
    }
}

fn table_string(table: &dyn toml_edit::TableLike, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn table_bool(table: &dyn toml_edit::TableLike, key: &str) -> Option<bool> {
    table.get(key).and_then(Item::as_bool)
}

const fn source_label(kind: DependencySourceKind) -> &'static str {
    match kind {
        DependencySourceKind::Registry => "registry",
        DependencySourceKind::Git => "Git",
        DependencySourceKind::Path => "path",
    }
}

fn dependency_location_label(kind: DependencyKind, target: &str) -> String {
    if target.is_empty() {
        format!("[{}]", kind.table_name())
    } else {
        format!("[target.{target}.{}]", kind.table_name())
    }
}

const fn default_true() -> bool {
    true
}

fn validate_hash(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Cargo manifest preview hash is invalid".to_owned());
    }
    Ok(())
}

fn sha256(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn sdk_path_literal() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates")
            .join("godot_rs");
        toml_edit::Value::from(path.to_string_lossy().as_ref()).to_string()
    }

    struct TempProject(PathBuf);

    impl TempProject {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "godot-rust-dependencies-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&root).expect("temporary project");
            std::fs::write(root.join("project.godot"), b"[application]\n").expect("project");
            std::fs::create_dir(root.join("src")).expect("src");
            std::fs::write(root.join("src/lib.rs"), b"").expect("lib");
            std::fs::write(
                root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                     \n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\
                     \n[dependencies]\ngodot_rs = {{ path = {} }}\nserde = \"1\"\n\
                     \n[package.metadata.godot-rust]\nenabled = true\ngodot = \"4.4\"\n",
                    sdk_path_literal()
                ),
            )
            .expect("manifest");
            Self(root)
        }

        fn workspace() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "godot-rust-workspace-dependencies-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("game/src")).expect("workspace member");
            std::fs::write(root.join("project.godot"), b"[application]\n").expect("project");
            std::fs::write(root.join("game/src/lib.rs"), b"").expect("lib");
            std::fs::write(
                root.join("Cargo.toml"),
                "[workspace]\nmembers = [\"game\"]\nresolver = \"3\"\n\
                 \n[workspace.dependencies]\n\
                 serde_alias = { package = \"serde\", version = \"1\", features = [\"std\"] }\n\
                 \n[workspace.metadata.godot-rust]\npackage = \"game\"\n",
            )
            .expect("workspace manifest");
            std::fs::write(
                root.join("game/Cargo.toml"),
                format!(
                    "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                     \n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n\
                     \n[dependencies]\ngodot_rs = {{ path = {} }}\n\
                     serde_alias = {{ workspace = true, features = [\"derive\"] }}\n\
                     \n[target.'cfg(target_arch = \"wasm32\")'.dependencies]\nweb-sys = \"0.3\"\n\
                     \n[package.metadata.godot-rust]\nenabled = true\ngodot = \"4.4\"\n",
                    sdk_path_literal()
                ),
            )
            .expect("member manifest");
            Self(root)
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn registry_change() -> DependencyChange {
        DependencyChange {
            operation: DependencyOperation::Upsert,
            kind: DependencyKind::Normal,
            target: String::new(),
            name: "serde".to_owned(),
            requirement: "1.0.200".to_owned(),
            source_kind: DependencySourceKind::Registry,
            source: String::new(),
            git_reference_kind: String::new(),
            git_reference: String::new(),
            features: vec!["derive".to_owned()],
            default_features: true,
            optional: false,
            allow_source_change: false,
        }
    }

    #[test]
    fn dependency_changes_are_previewed_hash_guarded_and_format_preserving() {
        let project = TempProject::new();
        let listed = list_dependencies(&project.0, None::<&str>).expect("list");
        assert!(
            listed
                .dependencies
                .iter()
                .any(|entry| entry.name == "serde")
        );
        let preview = preview_dependency_change(&project.0, registry_change(), None::<&str>)
            .expect("preview");
        assert_eq!(preview.before, " \"1\"");
        assert!(preview.after.contains("features"));
        apply_dependency_change(
            &project.0,
            &preview.expected_sha256,
            preview.change,
            None::<&str>,
        )
        .expect("apply");
        let manifest = std::fs::read_to_string(project.0.join("Cargo.toml")).expect("manifest");
        assert!(manifest.contains("serde = { version = \"1.0.200\", features = [\"derive\"] }"));
        assert!(manifest.contains("[package.metadata.godot-rust]"));
    }

    #[test]
    fn source_conflicts_and_stale_previews_do_not_modify_the_manifest() {
        let project = TempProject::new();
        let mut change = registry_change();
        change.source_kind = DependencySourceKind::Git;
        change.source = "https://example.com/serde.git".to_owned();
        let error = preview_dependency_change(&project.0, change, None::<&str>)
            .expect_err("source conflict");
        assert!(error.contains("source replacement"));
        let preview = preview_dependency_change(&project.0, registry_change(), None::<&str>)
            .expect("preview");
        let manifest_path = project.0.join("Cargo.toml");
        let mut manifest = std::fs::read_to_string(&manifest_path).expect("manifest");
        manifest.push_str("\n# changed after preview\n");
        std::fs::write(&manifest_path, manifest).expect("external change");
        let error = apply_dependency_change(
            &project.0,
            &preview.expected_sha256,
            preview.change,
            None::<&str>,
        )
        .expect_err("stale");
        assert!(error.contains("changed after"));
    }

    #[test]
    fn workspace_dependencies_preserve_aliases_and_shared_options() {
        let project = TempProject::workspace();
        let listed = list_dependencies(&project.0, None::<&str>).expect("list");
        let inherited = listed
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "serde_alias")
            .expect("inherited dependency");
        assert!(inherited.inherited);
        assert_eq!(inherited.package.as_deref(), Some("serde"));
        assert_eq!(inherited.requirement.as_deref(), Some("1"));
        assert_eq!(inherited.features, ["derive"]);

        let mut change = registry_change();
        change.name = "serde_alias".to_owned();
        change.requirement = "1.0.210".to_owned();
        change.features = vec!["derive".to_owned(), "rc".to_owned()];
        let preview =
            preview_dependency_change(&project.0, change, None::<&str>).expect("workspace preview");
        assert!(preview.before.contains("Workspace entry"));
        apply_dependency_change(
            &project.0,
            &preview.expected_sha256,
            preview.change,
            None::<&str>,
        )
        .expect("workspace apply");

        let workspace = std::fs::read_to_string(project.0.join("Cargo.toml")).expect("workspace");
        assert!(workspace.contains("package = \"serde\""));
        assert!(workspace.contains("version = \"1.0.210\""));
        assert!(workspace.contains("features = [\"std\"]"));
        let member =
            std::fs::read_to_string(project.0.join("game/Cargo.toml")).expect("member manifest");
        assert!(member.contains("workspace = true"));
        assert!(member.contains("features = [\"derive\", \"rc\"]"));
    }

    #[test]
    fn target_specific_dependencies_are_listed_and_changed_in_place() {
        let project = TempProject::workspace();
        let listed = list_dependencies(&project.0, None::<&str>).expect("list");
        let target = listed
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "web-sys")
            .expect("target dependency");
        assert_eq!(
            target.target.as_deref(),
            Some("cfg(target_arch = \"wasm32\")")
        );

        let mut change = registry_change();
        change.name = "web-sys".to_owned();
        change.requirement = "0.3.77".to_owned();
        change.target = "cfg(target_arch = \"wasm32\")".to_owned();
        change.features.clear();
        let preview =
            preview_dependency_change(&project.0, change, None::<&str>).expect("target preview");
        apply_dependency_change(
            &project.0,
            &preview.expected_sha256,
            preview.change,
            None::<&str>,
        )
        .expect("target apply");
        let member =
            std::fs::read_to_string(project.0.join("game/Cargo.toml")).expect("member manifest");
        assert!(member.contains("web-sys = \"0.3.77\""));
        assert_eq!(member.matches("web-sys").count(), 1);
    }
}
