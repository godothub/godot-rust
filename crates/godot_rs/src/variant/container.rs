use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;
use std::rc::Rc;

use godot_api::abi::*;

use super::{Variant, VariantConvert, VariantError};
use crate::engine::{EngineReturn, builtin, utility};
use crate::error::{EngineError, EngineResult};

enum Remote {
    Host {
        token: u64,
        _owner: crate::module::HostContainerToken,
    },
    Native(Rc<crate::native::NativeContainerToken>),
}

struct Container {
    kind: AbiValueType,
    element: Option<&'static str>,
    remote: OnceCell<Rc<Remote>>,
    wire: OnceCell<Box<[u8]>>,
    empty_wire: OnceCell<Box<[u8]>>,
    initializer: RefCell<Option<Vec<Variant>>>,
}

impl Container {
    fn empty(kind: AbiValueType, element: Option<&'static str>) -> Rc<Self> {
        Rc::new(Self {
            kind,
            element,
            remote: OnceCell::new(),
            wire: OnceCell::new(),
            empty_wire: OnceCell::new(),
            initializer: RefCell::new(Some(Vec::new())),
        })
    }
    fn from_host(kind: AbiValueType, token: u64) -> EngineResult<Rc<Self>> {
        let owner = crate::module::retain_container(kind, token)?;
        Ok(Self::from_remote(
            kind,
            Remote::Host {
                token,
                _owner: owner,
            },
        ))
    }
    fn from_native(kind: AbiValueType, value: Rc<crate::native::NativeContainerToken>) -> Rc<Self> {
        Self::from_remote(kind, Remote::Native(value))
    }
    fn from_remote(kind: AbiValueType, value: Remote) -> Rc<Self> {
        let result = Self::empty(kind, None);
        let _ = result.remote.set(Rc::new(value));
        result
    }
    fn remote(&self) -> EngineResult<&Rc<Remote>> {
        if let Some(remote) = self.remote.get() {
            return Ok(remote);
        }
        let bytes = super::encode_container_values(
            self.kind,
            self.initializer.borrow().as_deref().unwrap_or(&[]),
        )?;
        let remote =
            if crate::native::runtime::active_interface().is_some() {
                Rc::new(Remote::Native(crate::native::materialize_container(
                    self.kind,
                    &bytes,
                    self.element,
                )?))
            } else {
                let value = crate::module::container_access(
                    ABI_CONTAINER_MATERIALIZE,
                    self.kind,
                    0,
                    &bytes,
                    self.element,
                )?;
                let container = if self.kind == AbiValueType::ARRAY {
                    Array::<Variant>::__from_host_return(value)?.container
                } else {
                    Dictionary::__from_host_return(value)?.container
                };
                Rc::clone(container.remote.get().ok_or_else(|| {
                    EngineError::invalid_result("materialization omitted ownership")
                })?)
            };
        self.remote
            .set(remote)
            .map_err(|_| EngineError::invalid_result("container materialization reentered"))?;
        self.initializer.borrow_mut().take();
        Ok(self.remote.get().expect("initialized container"))
    }
    fn bytes(&self) -> Result<&[u8], VariantError> {
        if self.wire.get().is_none() {
            let remote = match self.remote() {
                Ok(remote) => remote,
                Err(error) if error.kind() == crate::error::EngineErrorKind::Unsupported => {
                    // Standalone validation inspects immutable construction
                    // data before an engine exists. Runtime accesses retry
                    // creation, shared by all cloned wrappers.
                    if self.empty_wire.get().is_none() {
                        let bytes = super::encode_container_values(
                            self.kind,
                            self.initializer.borrow().as_deref().unwrap_or(&[]),
                        )
                        .map_err(|_| VariantError::new("empty container encoding failed"))?;
                        let _ = self.empty_wire.set(bytes.into_boxed_slice());
                    }
                    return Ok(self.empty_wire.get().expect("empty container wire"));
                }
                Err(_) => return Err(VariantError::new("shared container materialization failed")),
            };
            let token = match remote.as_ref() {
                Remote::Host { token, .. } => *token,
                Remote::Native(value) => value.id(),
            };
            let mut bytes = Vec::with_capacity(44);
            bytes.extend_from_slice(&ABI_DYNAMIC_MAGIC);
            bytes.extend_from_slice(&ABI_DYNAMIC_VERSION.to_le_bytes());
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&0_u64.to_le_bytes());
            bytes.extend_from_slice(&self.kind.0.to_le_bytes());
            bytes.extend_from_slice(&ABI_DYNAMIC_NODE_CONTAINER.to_le_bytes());
            bytes.extend_from_slice(&8_u64.to_le_bytes());
            bytes.extend_from_slice(&token.to_le_bytes());
            let _ = self.wire.set(bytes.into_boxed_slice());
        }
        Ok(self.wire.get().expect("initialized container wire"))
    }
    fn snapshot(&self) -> EngineResult<Vec<Variant>> {
        if self.remote.get().is_none() {
            let values = self.initializer.borrow();
            if self.kind == AbiValueType::ARRAY || values.as_ref().is_none_or(Vec::is_empty) {
                return Ok(values.as_ref().cloned().unwrap_or_default());
            }
        }
        match self.remote()?.as_ref() {
            Remote::Native(value) => value.snapshot(),
            Remote::Host { token, .. } => {
                let result = crate::module::container_access(
                    ABI_CONTAINER_SNAPSHOT,
                    self.kind,
                    *token,
                    &[],
                    None,
                )?;
                super::decode_host_snapshot(result, self.kind)
            }
        }
    }
    fn assign(&self, values: &[Variant]) -> EngineResult<()> {
        let bytes = super::encode_container_values(self.kind, values)?;
        match self.remote()?.as_ref() {
            Remote::Native(value) => value.assign(&bytes),
            Remote::Host { token, .. } => {
                crate::module::container_access(
                    ABI_CONTAINER_ASSIGN,
                    self.kind,
                    *token,
                    &bytes,
                    self.element,
                )?;
                Ok(())
            }
        }
    }
}

/// A shared Godot Array. Clone shares identity; duplicate creates a copy.
/// Snapshot and to_vec explicitly copy elements into Rust-owned storage.
///
/// Containers are created lazily so default script fields can be
/// initialized before entering a callback. Engine operations are fallible
/// and must run on the Godot thread.
pub struct Array<T = Variant> {
    container: Rc<Container>,
    marker: PhantomData<T>,
}

impl<T: VariantConvert> Array<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            container: Container::empty(AbiValueType::ARRAY, T::__element_type()),
            marker: PhantomData,
        }
    }
    pub fn from_vec(values: Vec<T>) -> EngineResult<Self> {
        if values.len() > ABI_DYNAMIC_MAX_ELEMENTS {
            return Err(EngineError::invalid_argument(
                "Array initializer exceeds the element limit",
            ));
        }
        let result = Self::new();
        *result.container.initializer.borrow_mut() =
            Some(values.iter().map(VariantConvert::to_variant).collect());
        Ok(result)
    }
    pub fn len(&self) -> EngineResult<usize> {
        if self.container.remote.get().is_none() {
            return Ok(self
                .container
                .initializer
                .borrow()
                .as_ref()
                .map_or(0, Vec::len));
        }
        usize::try_from(builtin::array::size(&self.untyped())?)
            .map_err(|_| EngineError::invalid_result("Godot returned a negative Array size"))
    }
    pub fn is_empty(&self) -> EngineResult<bool> {
        Ok(self.len()? == 0)
    }
    pub fn get(&self, index: usize) -> EngineResult<Option<T>> {
        if self.container.remote.get().is_none() {
            return self
                .container
                .initializer
                .borrow()
                .as_ref()
                .and_then(|values| values.get(index))
                .cloned()
                .map(|value| {
                    T::from_variant(value).ok_or_else(|| {
                        EngineError::invalid_result("Array initializer has an incompatible element")
                    })
                })
                .transpose();
        }
        if index >= self.len()? {
            return Ok(None);
        }
        let value = builtin::array::get(&self.untyped(), index as i64)?;
        T::from_variant(value).map(Some).ok_or_else(|| {
            EngineError::invalid_result("Array element does not match its Rust type")
        })
    }
    pub fn set(&self, index: usize, value: T) -> EngineResult<()> {
        if index >= self.len()? {
            return Err(EngineError::invalid_argument(
                "Array index is out of bounds",
            ));
        }
        self.ensure_writable()?;
        builtin::array::set(&mut self.untyped(), index as i64, &value.to_variant())
    }
    pub fn push(&self, value: T) -> EngineResult<()> {
        self.ensure_writable()?;
        builtin::array::append(&mut self.untyped(), &value.to_variant())
    }
    pub fn pop(&self) -> EngineResult<Option<T>> {
        let len = self.len()?;
        if len == 0 {
            return Ok(None);
        }
        self.remove(len - 1)
    }
    pub fn insert(&self, index: usize, value: T) -> EngineResult<()> {
        if index > self.len()? {
            return Err(EngineError::invalid_argument(
                "Array insert index is out of bounds",
            ));
        }
        self.ensure_writable()?;
        let error = builtin::array::insert(&mut self.untyped(), index as i64, &value.to_variant())?;
        if error != 0 {
            return Err(EngineError::invalid_argument(
                "Godot rejected Array insertion",
            ));
        }
        Ok(())
    }
    pub fn remove(&self, index: usize) -> EngineResult<Option<T>> {
        let Some(value) = self.get(index)? else {
            return Ok(None);
        };
        self.ensure_writable()?;
        builtin::array::remove_at(&mut self.untyped(), index as i64)?;
        Ok(Some(value))
    }
    pub fn clear(&self) -> EngineResult<()> {
        self.ensure_writable()?;
        builtin::array::clear(&mut self.untyped())
    }
    pub fn is_read_only(&self) -> EngineResult<bool> {
        if self.container.remote.get().is_none() {
            return Ok(false);
        }
        builtin::array::is_read_only(&self.untyped())
    }
    pub fn make_read_only(&self) -> EngineResult<()> {
        builtin::array::make_read_only(&mut self.untyped())
    }
    fn ensure_writable(&self) -> EngineResult<()> {
        if self.is_read_only()? {
            Err(EngineError::invalid_argument("Godot Array is read-only"))
        } else {
            Ok(())
        }
    }
    pub fn duplicate(&self, deep: bool) -> EngineResult<Self> {
        Self::try_from_untyped(builtin::array::duplicate(&self.untyped(), deep)?)
    }
    pub fn is_same<U: VariantConvert>(&self, other: &Array<U>) -> EngineResult<bool> {
        if Rc::ptr_eq(&self.container, &other.container) {
            return Ok(true);
        }
        utility::is_same(&self.to_variant(), &other.to_variant())
    }
    pub fn equals<U: VariantConvert>(&self, other: &Array<U>) -> EngineResult<bool> {
        builtin::array::operator_equal_array_4(&self.untyped(), &other.untyped())
    }
    /// Copies elements in one runtime access. Nested containers keep their
    /// shared identity; objects are not recursively duplicated.
    pub fn to_vec(&self) -> EngineResult<Vec<T>> {
        self.container
            .snapshot()?
            .into_iter()
            .map(|value| {
                T::from_variant(value).ok_or_else(|| {
                    EngineError::invalid_result("snapshot element has the wrong Rust type")
                })
            })
            .collect()
    }
    pub fn snapshot(&self) -> EngineResult<ArraySnapshot<T>> {
        self.to_vec().map(ArraySnapshot::from_vec)
    }
    /// Replaces contents while preserving the existing container identity.
    pub fn assign_from_slice(&self, values: &[T]) -> EngineResult<()> {
        self.ensure_writable()?;
        let values = values
            .iter()
            .map(VariantConvert::to_variant)
            .collect::<Vec<_>>();
        self.container.assign(&values)
    }
    pub fn try_as<U: VariantConvert>(&self) -> EngineResult<Array<U>> {
        Array::<U>::try_from_untyped(self.untyped())
    }
    /// Shares the same engine container, retaining all engine type constraints.
    pub fn to_untyped(&self) -> Array {
        self.untyped()
    }
    /// Creates an independent typed container after checking every element.
    pub fn typed_copy<U: VariantConvert>(&self) -> EngineResult<Array<U>> {
        let values = self
            .container
            .snapshot()?
            .into_iter()
            .map(|value| {
                U::from_variant(value).ok_or_else(|| {
                    EngineError::invalid_argument("typed copy contains an incompatible value")
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;
        Array::<U>::from_vec(values)
    }
    pub(super) fn try_from_untyped(value: Array) -> EngineResult<Self> {
        if let Some(element) = T::__element_type() {
            let actual = builtin::array::get_typed_builtin(&value)?;
            let expected = super::builtin_type_id(element).unwrap_or(24);
            if actual != expected {
                return Err(EngineError::invalid_argument(
                    "Array has incompatible engine element constraints",
                ));
            }
            if expected == 24 && builtin::array::get_typed_class_name(&value)?.as_str() != element {
                return Err(EngineError::invalid_argument(
                    "Array has incompatible engine class constraints",
                ));
            }
        }
        Ok(Self {
            container: value.container,
            marker: PhantomData,
        })
    }
    pub(super) fn untyped(&self) -> Array {
        Array {
            container: Rc::clone(&self.container),
            marker: PhantomData,
        }
    }
    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], VariantError> {
        self.container.bytes()
    }
    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::from_variant(Variant::__from_bytes(bytes)?)
    }
    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        Self::from_variant(Variant::__from_host_bytes(bytes, ownership)?)
    }
    pub(super) fn from_host(token: u64) -> EngineResult<Self> {
        Ok(Self {
            container: Container::from_host(AbiValueType::ARRAY, token)?,
            marker: PhantomData,
        })
    }
    pub(crate) fn from_native(token: Rc<crate::native::NativeContainerToken>) -> Self {
        Self {
            container: Container::from_native(AbiValueType::ARRAY, token),
            marker: PhantomData,
        }
    }
    pub(crate) fn native_token(&self) -> EngineResult<Rc<crate::native::NativeContainerToken>> {
        match self.container.remote()?.as_ref() {
            Remote::Native(value) => Ok(Rc::clone(value)),
            _ => Err(EngineError::invalid_argument(
                "container belongs to another runtime",
            )),
        }
    }
}
impl<T> Clone for Array<T> {
    fn clone(&self) -> Self {
        Self {
            container: Rc::clone(&self.container),
            marker: PhantomData,
        }
    }
}
impl<T: VariantConvert> Default for Array<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> fmt::Debug for Array<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Array")
            .field("shared", &Rc::as_ptr(&self.container))
            .finish()
    }
}
impl<T: VariantConvert> PartialEq for Array<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.container, &other.container)
            || builtin::array::operator_equal_array_4(&self.untyped(), &other.untyped())
                .unwrap_or(false)
    }
}

/// An explicit Rust-owned, shallow element snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ArraySnapshot<T = Variant> {
    values: Vec<T>,
}
impl<T> ArraySnapshot<T> {
    pub fn from_vec(values: Vec<T>) -> Self {
        Self { values }
    }
    pub fn as_slice(&self) -> &[T] {
        &self.values
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values
    }
    pub fn into_vec(self) -> Vec<T> {
        self.values
    }
}
impl<T: VariantConvert> ArraySnapshot<T> {
    pub fn into_array(self) -> EngineResult<Array<T>> {
        Array::from_vec(self.values)
    }
}

/// A shared Godot Dictionary, preserving engine hashing, key equality,
/// insertion order, type constraints and identity across script reload.
pub struct Dictionary {
    container: Rc<Container>,
}
impl Dictionary {
    #[must_use]
    pub fn new() -> Self {
        Self {
            container: Container::empty(AbiValueType::DICTIONARY, None),
        }
    }
    pub fn from_entries(entries: Vec<(Variant, Variant)>) -> EngineResult<Self> {
        if entries.len() > ABI_DYNAMIC_MAX_ELEMENTS {
            return Err(EngineError::invalid_argument(
                "Dictionary initializer exceeds the element limit",
            ));
        }
        let result = Self::new();
        *result.container.initializer.borrow_mut() = Some(
            entries
                .into_iter()
                .flat_map(|(key, value)| [key, value])
                .collect(),
        );
        Ok(result)
    }
    pub fn len(&self) -> EngineResult<usize> {
        if self.container.remote.get().is_none()
            && self
                .container
                .initializer
                .borrow()
                .as_ref()
                .is_none_or(Vec::is_empty)
        {
            return Ok(0);
        }
        usize::try_from(builtin::dictionary::size(self)?)
            .map_err(|_| EngineError::invalid_result("Godot returned a negative Dictionary size"))
    }
    pub fn is_empty(&self) -> EngineResult<bool> {
        Ok(self.len()? == 0)
    }
    pub fn get(&self, key: &Variant) -> EngineResult<Option<Variant>> {
        if !builtin::dictionary::has(self, key)? {
            return Ok(None);
        }
        builtin::dictionary::keyed_get(self, key).map(Some)
    }
    pub fn insert(
        &self,
        key: impl Into<Variant>,
        value: impl Into<Variant>,
    ) -> EngineResult<Option<Variant>> {
        self.ensure_writable()?;
        let key = key.into();
        let value = value.into();
        let previous = self.get(&key)?;
        if !builtin::dictionary::set(&mut self.clone(), &key, &value)? {
            return Err(EngineError::invalid_argument(
                "Godot rejected Dictionary key or value",
            ));
        }
        Ok(previous)
    }
    pub fn remove(&self, key: &Variant) -> EngineResult<Option<Variant>> {
        self.ensure_writable()?;
        let previous = self.get(key)?;
        builtin::dictionary::erase(&mut self.clone(), key)?;
        Ok(previous)
    }
    pub fn clear(&self) -> EngineResult<()> {
        self.ensure_writable()?;
        builtin::dictionary::clear(&mut self.clone())
    }
    pub fn is_read_only(&self) -> EngineResult<bool> {
        if self.container.remote.get().is_none() {
            return Ok(false);
        }
        builtin::dictionary::is_read_only(self)
    }
    pub fn make_read_only(&self) -> EngineResult<()> {
        builtin::dictionary::make_read_only(&mut self.clone())
    }
    fn ensure_writable(&self) -> EngineResult<()> {
        if self.is_read_only()? {
            Err(EngineError::invalid_argument(
                "Godot Dictionary is read-only",
            ))
        } else {
            Ok(())
        }
    }
    pub fn duplicate(&self, deep: bool) -> EngineResult<Self> {
        builtin::dictionary::duplicate(self, deep)
    }
    pub fn is_same(&self, other: &Self) -> EngineResult<bool> {
        if Rc::ptr_eq(&self.container, &other.container) {
            return Ok(true);
        }
        utility::is_same(&self.to_variant(), &other.to_variant())
    }
    pub fn equals(&self, other: &Self) -> EngineResult<bool> {
        builtin::dictionary::operator_equal_dictionary_3(self, other)
    }
    /// Converts to Rust hashing/equality rules. Colliding converted keys are
    /// rejected instead of silently losing an engine entry.
    pub fn to_hash_map<K: VariantConvert + Eq + Hash, V: VariantConvert>(
        &self,
    ) -> EngineResult<HashMap<K, V>> {
        self.snapshot()?.into_hash_map()
    }
    pub fn from_hash_map<K: VariantConvert + Eq + Hash, V: VariantConvert>(
        values: HashMap<K, V>,
    ) -> EngineResult<Self> {
        Self::from_entries(
            values
                .into_iter()
                .map(|(key, value)| (key.to_variant(), value.to_variant()))
                .collect(),
        )
    }
    pub fn snapshot(&self) -> EngineResult<DictionarySnapshot> {
        let values = self.container.snapshot()?;
        let mut values = values.into_iter();
        let mut entries = Vec::new();
        while let Some(key) = values.next() {
            let value = values.next().ok_or_else(|| {
                EngineError::invalid_result("Dictionary snapshot omitted a value")
            })?;
            entries.push((key, value));
        }
        Ok(DictionarySnapshot { entries })
    }
    pub fn assign_from_entries(&self, entries: &[(Variant, Variant)]) -> EngineResult<()> {
        self.ensure_writable()?;
        let values = entries
            .iter()
            .flat_map(|(key, value)| [key.clone(), value.clone()])
            .collect::<Vec<_>>();
        self.container.assign(&values)
    }
    #[doc(hidden)]
    pub fn __bytes(&self) -> Result<&[u8], VariantError> {
        self.container.bytes()
    }
    #[doc(hidden)]
    pub fn __from_bytes(bytes: &[u8]) -> Option<Self> {
        Self::from_variant(Variant::__from_bytes(bytes)?)
    }
    pub(crate) fn __from_host_bytes(
        bytes: &[u8],
        ownership: Option<crate::module::HostDynamicValueToken>,
    ) -> Option<Self> {
        Self::from_variant(Variant::__from_host_bytes(bytes, ownership)?)
    }
    pub(super) fn from_host(token: u64) -> EngineResult<Self> {
        Ok(Self {
            container: Container::from_host(AbiValueType::DICTIONARY, token)?,
        })
    }
    pub(crate) fn from_native(token: Rc<crate::native::NativeContainerToken>) -> Self {
        Self {
            container: Container::from_native(AbiValueType::DICTIONARY, token),
        }
    }
    pub(crate) fn native_token(&self) -> EngineResult<Rc<crate::native::NativeContainerToken>> {
        match self.container.remote()?.as_ref() {
            Remote::Native(value) => Ok(Rc::clone(value)),
            _ => Err(EngineError::invalid_argument(
                "container belongs to another runtime",
            )),
        }
    }
}
impl Clone for Dictionary {
    fn clone(&self) -> Self {
        Self {
            container: Rc::clone(&self.container),
        }
    }
}
impl Default for Dictionary {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dictionary")
            .field("shared", &Rc::as_ptr(&self.container))
            .finish()
    }
}
impl PartialEq for Dictionary {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.container, &other.container)
            || builtin::dictionary::operator_equal_dictionary_3(self, other).unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DictionarySnapshot {
    entries: Vec<(Variant, Variant)>,
}
impl DictionarySnapshot {
    pub fn from_entries(entries: Vec<(Variant, Variant)>) -> Self {
        Self { entries }
    }
    pub fn as_entries(&self) -> &[(Variant, Variant)] {
        &self.entries
    }
    pub fn into_entries(self) -> Vec<(Variant, Variant)> {
        self.entries
    }
    pub fn into_dictionary(self) -> EngineResult<Dictionary> {
        Dictionary::from_entries(self.entries)
    }
    pub fn into_hash_map<K: VariantConvert + Eq + Hash, V: VariantConvert>(
        self,
    ) -> EngineResult<HashMap<K, V>> {
        let mut result = HashMap::with_capacity(self.entries.len());
        for (key, value) in self.entries {
            let key = K::from_variant(key).ok_or_else(|| {
                EngineError::invalid_argument("Dictionary key has an incompatible Rust type")
            })?;
            let value = V::from_variant(value).ok_or_else(|| {
                EngineError::invalid_argument("Dictionary value has an incompatible Rust type")
            })?;
            if result.insert(key, value).is_some() {
                return Err(EngineError::invalid_argument(
                    "Dictionary keys collide under Rust conversion",
                ));
            }
        }
        Ok(result)
    }
}

macro_rules! packed_conversion {
    ($element:ty, $packed:ty, $module:ident, $constructor:ident) => {
        impl Array<$element> {
            /// Converts in the engine, without a Rust Variant per packed element.
            pub fn from_packed(values: &$packed) -> EngineResult<Self> {
                let array = builtin::array::$constructor(values)?;
                let type_id = super::builtin_type_id(
                    <$element as VariantConvert>::__element_type().expect("packed element type"),
                )
                .expect("builtin packed element type");
                let typed = builtin::array::construct_2(&array, type_id, "", &Variant::nil())?;
                Self::try_from_untyped(typed)
            }
            /// Reads a packed buffer in one engine operation.
            pub fn to_packed(&self) -> EngineResult<$packed> {
                builtin::$module::construct_2(&self.untyped())
            }
        }
    };
}
packed_conversion!(
    i64,
    crate::packed_array::PackedInt64Array,
    packed_int64_array,
    construct_5
);
packed_conversion!(
    f64,
    crate::packed_array::PackedFloat64Array,
    packed_float64_array,
    construct_7
);
packed_conversion!(
    crate::math::Vector2,
    crate::packed_array::PackedVector2Array,
    packed_vector2_array,
    construct_9
);
packed_conversion!(
    crate::math::Vector3,
    crate::packed_array::PackedVector3Array,
    packed_vector3_array,
    construct_10
);
packed_conversion!(
    crate::math::Color,
    crate::packed_array::PackedColorArray,
    packed_color_array,
    construct_11
);
packed_conversion!(
    crate::math::Vector4,
    crate::packed_array::PackedVector4Array,
    packed_vector4_array,
    construct_12
);
