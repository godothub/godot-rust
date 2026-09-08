use super::*;

impl<T: crate::script::ScriptClass> ObjectRef<T> {
    /// Accesses script members explicitly, including names shared with engine methods.
    pub fn script(&self) -> &T::Api {
        self
    }
}

impl<T: crate::script::ScriptClass> GodotRef<T> {
    /// Accesses script members explicitly, including names shared with engine methods.
    pub fn script(&self) -> &T::Api {
        self
    }
}

/// Loads a resource and checks its engine class and attached Rust script.
pub fn load<T: GodotClass + Inherits<Resource>>(path: &str) -> EngineResult<GodotRef<T>> {
    let loader = ResourceLoader::singleton()?;
    let resource = ResourceLoaderApi::load(
        &loader,
        path,
        "",
        resource_loader::CacheMode::CACHE_MODE_REUSE,
    )?
    .ok_or_else(|| {
        EngineError::invalid_argument(format!("resource `{path}` could not be loaded"))
    })?;
    resource.try_cast()
}

impl<T: GodotClass + Inherits<Node>> ObjectRef<T> {
    /// Gets a required child node with a checked engine or Rust script type.
    pub fn node<U: GodotClass + Inherits<Node>>(self, path: &str) -> EngineResult<NodeRef<U>> {
        self.try_node(path)?.ok_or_else(|| {
            EngineError::invalid_argument(format!("required node `{path}` was not found"))
        })
    }

    /// Returns None only when absent; wrong classes and scripts are errors.
    pub fn try_node<U: GodotClass + Inherits<Node>>(
        self,
        path: &str,
    ) -> EngineResult<Option<NodeRef<U>>> {
        NodeApi::get_node_or_null(&self, path)?
            .map(ObjectRef::try_cast)
            .transpose()
    }
}

impl<T: GodotClass + Inherits<Node>> Base<T> {
    pub fn node<U: GodotClass + Inherits<Node>>(self, path: &str) -> EngineResult<NodeRef<U>> {
        self.object_ref()?.node(path)
    }

    pub fn try_node<U: GodotClass + Inherits<Node>>(
        self,
        path: &str,
    ) -> EngineResult<Option<NodeRef<U>>> {
        self.object_ref()?.try_node(path)
    }
}
