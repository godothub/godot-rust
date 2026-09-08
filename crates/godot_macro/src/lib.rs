use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::visit_mut::VisitMut;
use syn::{
    Attribute, Error, Expr, Fields, FnArg, GenericArgument, ImplItem, ImplItemConst, ImplItemFn,
    Item, ItemEnum, ItemImpl, ItemStruct, LitStr, Meta, Pat, Path, PathArguments, ReturnType,
    Token, Type, Visibility,
};

mod typed_script;

/// Declares an attachable Rust Script or its Godot-facing impl block.
#[proc_macro_attribute]
pub fn script(arguments: TokenStream, input: TokenStream) -> TokenStream {
    let arguments = TokenStream2::from(arguments);
    let item = syn::parse_macro_input!(input as Item);
    let expanded = match item {
        Item::Struct(item) => expand_struct(arguments, item),
        Item::Impl(item) => expand_impl(arguments, item),
        other => Err(Error::new_spanned(
            other,
            "`#[script]` can only be used on a struct or its inherent impl block",
        )),
    };
    expanded.unwrap_or_else(Error::into_compile_error).into()
}

/// Declares a forward-compatible, integer-backed Godot enum.
#[proc_macro_attribute]
pub fn godot_enum(arguments: TokenStream, input: TokenStream) -> TokenStream {
    expand_custom_integer(arguments, input, false)
}

/// Declares forward-compatible, integer-backed Godot flags.
#[proc_macro_attribute]
pub fn godot_flags(arguments: TokenStream, input: TokenStream) -> TokenStream {
    expand_custom_integer(arguments, input, true)
}

fn expand_custom_integer(arguments: TokenStream, input: TokenStream, flags: bool) -> TokenStream {
    let arguments = TokenStream2::from(arguments);
    if !arguments.is_empty() {
        return Error::new_spanned(arguments, "this attribute does not take arguments")
            .into_compile_error()
            .into();
    }
    let item = syn::parse_macro_input!(input as ItemEnum);
    custom_integer_tokens(item, flags)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn custom_integer_tokens(item: ItemEnum, flags: bool) -> syn::Result<TokenStream2> {
    if !item.generics.params.is_empty() {
        return Err(Error::new_spanned(
            item.generics,
            "Godot enums and flags cannot have generic parameters",
        ));
    }
    if item.variants.is_empty() {
        return Err(Error::new_spanned(
            item.ident,
            "a Godot enum or flag type requires at least one named value",
        ));
    }
    let mut type_documentation = Vec::new();
    let mut type_conditions = Vec::new();
    for attribute in item.attrs {
        if attribute.path().is_ident("doc") {
            type_documentation.push(attribute);
        } else if attribute.path().is_ident("cfg") {
            type_conditions.push(attribute);
        } else if attribute.path().is_ident("repr") {
            // The generated public type always uses the ABI-safe transparent
            // representation selected by this macro.
        } else {
            return Err(Error::new_spanned(
                attribute,
                "only documentation and `cfg` attributes are supported here; the macro supplies the required derives and representation",
            ));
        }
    }

    let mut values = Vec::with_capacity(item.variants.len());
    let mut explicit_default = None;
    for (index, variant) in item.variants.into_iter().enumerate() {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(Error::new_spanned(
                variant.fields,
                "Godot enum and flag values cannot contain fields",
            ));
        }
        let Some((_, expression)) = variant.discriminant else {
            return Err(Error::new_spanned(
                variant.ident,
                "assign an explicit integer value to every Godot enum or flag entry",
            ));
        };
        let mut documentation = Vec::new();
        let mut conditions = Vec::new();
        let mut is_default = false;
        for attribute in variant.attrs {
            if attribute.path().is_ident("default") {
                if flags {
                    return Err(Error::new_spanned(
                        attribute,
                        "Godot flag types default to the empty set; choose a field default with `#[export(default = ...)]`",
                    ));
                }
                if is_default {
                    return Err(Error::new_spanned(
                        attribute,
                        "duplicate `default` attribute",
                    ));
                }
                is_default = true;
            } else if attribute.path().is_ident("doc") {
                documentation.push(attribute);
            } else if attribute.path().is_ident("cfg") {
                conditions.push(attribute);
            } else {
                return Err(Error::new_spanned(
                    attribute,
                    "Godot enum values only support documentation, `cfg`, and `default` attributes",
                ));
            }
        }
        if is_default && explicit_default.replace(index).is_some() {
            return Err(Error::new_spanned(
                &variant.ident,
                "only one Godot enum value can be the default",
            ));
        }
        let option_name = LitStr::new(
            &screaming_snake_case(&variant.ident.to_string()),
            variant.ident.span(),
        );
        values.push((
            variant.ident,
            expression,
            documentation,
            conditions,
            option_name,
        ));
    }

    let visibility = item.vis;
    let name = item.ident;
    let raw_type = if flags { quote!(u64) } else { quote!(i64) };
    let signed = !flags;
    let default_index = explicit_default.unwrap_or(0);
    let default_expression = &values[default_index].1;
    let default_raw = if flags {
        quote!(0_u64)
    } else {
        quote!((#default_expression) as u64)
    };
    let constants = values
        .iter()
        .map(|(ident, expression, documentation, conditions, _)| {
            quote! {
                #(#documentation)*
                #(#conditions)*
                #[allow(non_upper_case_globals)]
                pub const #ident: Self = Self((#expression) as #raw_type);
            }
        });
    let property_options = values
        .iter()
        .map(|(_, expression, _, conditions, option_name)| {
            quote! {
                #(#conditions)*
                ::godot_rs::abi::AbiGodotIntegerOptionV1 {
                    name: ::godot_rs::abi::AbiByteSlice::from_static(#option_name),
                    raw: (#expression) as u64,
                }
            }
        });
    let value_api = if flags {
        quote! {
            #[must_use]
            pub const fn from_bits_retain(bits: u64) -> Self { Self(bits) }

            #[must_use]
            pub const fn bits(self) -> u64 { self.0 }

            #[must_use]
            pub const fn empty() -> Self { Self(0) }

            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            #[must_use]
            pub const fn is_empty(self) -> bool { self.0 == 0 }
        }
    } else {
        quote! {
            #[must_use]
            pub const fn from_ord(value: i64) -> Self { Self(value) }

            #[must_use]
            pub const fn ord(self) -> i64 { self.0 }
        }
    };
    let conversions = if flags {
        quote! {
            #(#type_conditions)*
            impl ::core::convert::From<u64> for #name {
                fn from(value: u64) -> Self { Self::from_bits_retain(value) }
            }

            #(#type_conditions)*
            impl ::core::convert::From<#name> for u64 {
                fn from(value: #name) -> Self { value.bits() }
            }

            #(#type_conditions)*
            impl ::core::ops::BitOr for #name {
                type Output = Self;
                fn bitor(self, other: Self) -> Self { Self(self.0 | other.0) }
            }

            #(#type_conditions)*
            impl ::core::ops::BitOrAssign for #name {
                fn bitor_assign(&mut self, other: Self) { self.0 |= other.0; }
            }

            #(#type_conditions)*
            impl ::core::ops::BitAnd for #name {
                type Output = Self;
                fn bitand(self, other: Self) -> Self { Self(self.0 & other.0) }
            }

            #(#type_conditions)*
            impl ::core::ops::BitAndAssign for #name {
                fn bitand_assign(&mut self, other: Self) { self.0 &= other.0; }
            }

            #(#type_conditions)*
            impl ::core::ops::BitXor for #name {
                type Output = Self;
                fn bitxor(self, other: Self) -> Self { Self(self.0 ^ other.0) }
            }

            #(#type_conditions)*
            impl ::core::ops::BitXorAssign for #name {
                fn bitxor_assign(&mut self, other: Self) { self.0 ^= other.0; }
            }

            #(#type_conditions)*
            impl ::core::ops::Not for #name {
                type Output = Self;
                fn not(self) -> Self { Self(!self.0) }
            }
        }
    } else {
        quote! {
            #(#type_conditions)*
            impl ::core::convert::From<i64> for #name {
                fn from(value: i64) -> Self { Self::from_ord(value) }
            }

            #(#type_conditions)*
            impl ::core::convert::From<#name> for i64 {
                fn from(value: #name) -> Self { value.ord() }
            }
        }
    };
    let default_value = if flags {
        quote!(Self(0))
    } else {
        quote!(Self((#default_expression) as i64))
    };
    let raw = if flags {
        quote!(self.0)
    } else {
        quote!(self.0 as u64)
    };
    let from_raw = if flags {
        quote!(Self(raw))
    } else {
        quote!(Self(raw as i64))
    };

    Ok(quote! {
        #(#type_documentation)*
        #(#type_conditions)*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        #visibility struct #name(#raw_type);

        #(#type_conditions)*
        impl #name {
            #(#constants)*
            #value_api
        }

        #conversions

        #(#type_conditions)*
        impl ::core::default::Default for #name {
            fn default() -> Self { #default_value }
        }

        #(#type_conditions)*
        impl ::godot_rs::engine::GodotIntegerValue for #name {
            const SIGNED: bool = #signed;
            const PROPERTY_OPTIONS: &'static [::godot_rs::abi::AbiGodotIntegerOptionV1] = &[
                #(#property_options),*
            ];
            const PROPERTY_DEFAULT_RAW: u64 = #default_raw;

            fn __raw(self) -> u64 { #raw }
            fn __from_raw(raw: u64) -> Self { #from_raw }
        }

        #(#type_conditions)*
        impl ::godot_rs::engine::CustomGodotIntegerValue for #name {}
    })
}

fn screaming_snake_case(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut previous_lower_or_digit = false;
    for character in value.chars() {
        if character.is_ascii_uppercase() && previous_lower_or_digit {
            result.push('_');
        }
        if character == '-' || character == ' ' {
            if !result.ends_with('_') {
                result.push('_');
            }
            previous_lower_or_digit = false;
        } else {
            result.push(character.to_ascii_uppercase());
            previous_lower_or_digit = character.is_ascii_lowercase() || character.is_ascii_digit();
        }
    }
    result
}

#[derive(Default)]
struct ScriptArguments {
    base: Option<Path>,
    tool: bool,
    abstract_: bool,
    class_name: Option<String>,
    extends: Option<String>,
    icon: Option<String>,
}

impl Parse for ScriptArguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let tokens = input.parse::<TokenStream2>()?;
        let tokens = tokens
            .into_iter()
            .map(|token| match token {
                TokenTree::Ident(ident) if ident == "abstract" => {
                    TokenTree::Ident(Ident::new_raw("abstract", ident.span()))
                }
                token => token,
            })
            .collect();
        let entries = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(tokens)?;
        let mut result = Self::default();
        for entry in entries {
            match entry {
                Meta::NameValue(value) if value.path.is_ident("base") => {
                    if result.base.is_some() {
                        return Err(Error::new_spanned(value, "duplicate `base` argument"));
                    }
                    let Expr::Path(base) = value.value else {
                        return Err(Error::new_spanned(
                            value,
                            "`base` must be a Godot class name such as `Node2D`",
                        ));
                    };
                    result.base = Some(base.path);
                }
                Meta::Path(path) if path.is_ident("tool") => {
                    if result.tool {
                        return Err(Error::new_spanned(path, "duplicate `tool` argument"));
                    }
                    result.tool = true;
                }
                Meta::Path(path)
                    if path.leading_colon.is_none()
                        && path.segments.len() == 1
                        && path
                            .segments
                            .first()
                            .is_some_and(|segment| segment.ident == "r#abstract") =>
                {
                    if result.abstract_ {
                        return Err(Error::new_spanned(path, "duplicate `abstract` argument"));
                    }
                    result.abstract_ = true;
                }
                Meta::NameValue(value) if value.path.is_ident("class_name") => {
                    if result.class_name.is_some() {
                        return Err(Error::new_spanned(value, "duplicate `class_name` argument"));
                    }
                    let name = match value.value {
                        Expr::Path(path)
                            if path.path.leading_colon.is_none()
                                && path.path.segments.len() == 1 =>
                        {
                            path.path
                                .segments
                                .first()
                                .expect("single class-name segment")
                                .ident
                                .to_string()
                        }
                        Expr::Lit(literal) => {
                            let syn::Lit::Str(name) = literal.lit else {
                                return Err(Error::new_spanned(
                                    literal,
                                    "`class_name` must be an identifier or string",
                                ));
                            };
                            name.value()
                        }
                        other => {
                            return Err(Error::new_spanned(
                                other,
                                "`class_name` must be an identifier or string",
                            ));
                        }
                    };
                    if !is_godot_class_identifier(&name) {
                        return Err(Error::new_spanned(
                            value.path,
                            "`class_name` must be a valid Godot identifier",
                        ));
                    }
                    result.class_name = Some(name);
                }
                Meta::NameValue(value) if value.path.is_ident("extends") => {
                    if result.extends.is_some() {
                        return Err(Error::new_spanned(value, "duplicate `extends` argument"));
                    }
                    let Expr::Lit(literal) = value.value else {
                        return Err(Error::new_spanned(
                            value,
                            "`extends` must be a canonical `res://` Rust script path",
                        ));
                    };
                    let syn::Lit::Str(path) = literal.lit else {
                        return Err(Error::new_spanned(
                            literal,
                            "`extends` must be a canonical `res://` Rust script path",
                        ));
                    };
                    let path = path.value();
                    if !is_canonical_rust_resource_path(&path) {
                        return Err(Error::new_spanned(
                            path,
                            "`extends` must be a canonical `res://` Rust script path",
                        ));
                    }
                    result.extends = Some(path);
                }
                Meta::NameValue(value) if value.path.is_ident("icon") => {
                    if result.icon.is_some() {
                        return Err(Error::new_spanned(value, "duplicate `icon` argument"));
                    }
                    let Expr::Lit(literal) = value.value else {
                        return Err(Error::new_spanned(
                            value,
                            "`icon` must be a canonical `res://` resource path",
                        ));
                    };
                    let syn::Lit::Str(path) = literal.lit else {
                        return Err(Error::new_spanned(
                            literal,
                            "`icon` must be a canonical `res://` resource path",
                        ));
                    };
                    let path = path.value();
                    if !is_canonical_resource_path(&path) {
                        return Err(Error::new_spanned(
                            path,
                            "`icon` must be a canonical `res://` resource path",
                        ));
                    }
                    result.icon = Some(path);
                }
                unsupported => {
                    return Err(Error::new_spanned(
                        unsupported,
                        "supported script arguments are `base = ClassName`, `extends = \"res://path.rs\"`, `class_name = Name`, `icon = \"res://path.svg\"`, `tool`, and `abstract`",
                    ));
                }
            }
        }
        Ok(result)
    }
}

fn is_godot_class_identifier(value: &str) -> bool {
    let mut characters = value.bytes();
    matches!(characters.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && characters.all(|character| character.is_ascii_alphanumeric() || character == b'_')
}

fn is_canonical_rust_resource_path(value: &str) -> bool {
    is_canonical_resource_path(value) && value.ends_with(".rs")
}

fn is_canonical_resource_path(value: &str) -> bool {
    let Some(relative) = value.strip_prefix("res://") else {
        return false;
    };
    !relative.is_empty()
        && !relative.contains('\\')
        && relative
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FieldKind {
    Plain,
    Export,
    Property,
    Node,
    Signal,
}

struct FieldMetadata {
    kind: FieldKind,
    options: String,
    default: Option<Expr>,
    reload: ReloadPolicy,
    property_type: Option<PropertyType>,
    property_hint: u32,
    property_hint_string: String,
    property_hint_expression: Option<TokenStream2>,
    typed_array_element: Option<String>,
    typed_array_class: Option<Type>,
    property_object_class: Option<Type>,
    property_node_usage: bool,
    property_group: Option<String>,
    property_getter: Option<syn::Ident>,
    property_setter: Option<syn::Ident>,
    property_read_only: bool,
    node_path: Option<String>,
    node_class: Option<Type>,
    node_optional: bool,
    signal_arguments: Vec<SignalArgumentMetadata>,
}

impl Default for FieldMetadata {
    fn default() -> Self {
        Self {
            kind: FieldKind::Plain,
            options: String::new(),
            default: None,
            reload: ReloadPolicy::Default,
            property_type: None,
            property_hint: 0,
            property_hint_string: String::new(),
            property_hint_expression: None,
            typed_array_element: None,
            typed_array_class: None,
            property_object_class: None,
            property_node_usage: false,
            property_group: None,
            property_getter: None,
            property_setter: None,
            property_read_only: false,
            node_path: None,
            node_class: None,
            node_optional: false,
            signal_arguments: Vec::new(),
        }
    }
}

struct SignalArgumentMetadata {
    name: syn::Ident,
    type_: Type,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PropertyType {
    Bool,
    Int,
    Float,
    String,
    StringName,
    NodePath,
    Vector2,
    Vector2i,
    Vector3,
    Vector3i,
    Vector4,
    Vector4i,
    Rect2,
    Rect2i,
    Quaternion,
    Plane,
    Transform2D,
    Aabb,
    Basis,
    Transform3D,
    Projection,
    Color,
    Array,
    Dictionary,
    PackedByteArray,
    PackedInt32Array,
    PackedInt64Array,
    PackedFloat32Array,
    PackedFloat64Array,
    PackedStringArray,
    PackedVector2Array,
    PackedVector3Array,
    PackedColorArray,
    PackedVector4Array,
    NodeRef,
    ResourceRef,
    GodotInteger,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ReloadPolicy {
    Default,
    Persist,
    Skip,
}

fn expand_struct(arguments: TokenStream2, mut item: ItemStruct) -> syn::Result<TokenStream2> {
    let arguments = syn::parse2::<ScriptArguments>(arguments)?;
    let Some(base) = arguments.base else {
        return Err(Error::new_spanned(
            &item.ident,
            "attachable scripts require `#[script(base = GodotClass)]`",
        ));
    };
    if base.leading_colon.is_some() || base.segments.len() != 1 {
        return Err(Error::new_spanned(
            &base,
            "`base` must be one Godot class name such as `Node2D`",
        ));
    }
    let base_type = base
        .segments
        .first()
        .expect("single-segment base was validated")
        .ident
        .clone();
    if !item.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &item.generics,
            "attachable scripts cannot have generic parameters",
        ));
    }

    let mut descriptors = Vec::new();
    let mut initializers = Vec::new();
    let mut field_getters = Vec::new();
    let mut field_setters = Vec::new();
    let mut property_proxies = Vec::new();
    let mut signal_proxies = Vec::new();
    let name = &item.ident;
    let self_type: Type = syn::parse_quote!(#name);
    match &mut item.fields {
        Fields::Named(fields) => {
            for (index, field) in fields.named.iter_mut().enumerate() {
                let Some(name) = field.ident.as_ref() else {
                    return Err(Error::new_spanned(field, "script field must have a name"));
                };
                if name == "__godot_rs_tasks" {
                    return Err(Error::new_spanned(
                        name,
                        "this field name is reserved for the script task scope",
                    ));
                }
                let mut generated_type = field.ty.clone();
                ExplicitSelfType(&self_type).visit_type_mut(&mut generated_type);
                let metadata = take_field_metadata(&mut field.attrs, &generated_type)?;
                let index = u32::try_from(index).map_err(|_| {
                    Error::new_spanned(name, "script has too many fields for the project ABI")
                })?;
                let initializer = if metadata.kind == FieldKind::Signal {
                    let rust_name = name.to_string();
                    let godot_name = rust_name.strip_prefix("r#").unwrap_or(&rust_name);
                    let godot_name = LitStr::new(godot_name, name.span());
                    quote!(::godot_rs::signal::Signal::new(#index, #godot_name))
                } else if metadata.kind == FieldKind::Node {
                    let class = metadata.node_class.as_ref().expect("validated node class");
                    quote!({
                        ::godot_rs::script::assert_export_node::<#class>();
                        ::core::default::Default::default()
                    })
                } else if metadata.property_type == Some(PropertyType::GodotInteger) {
                    let field_type = &generated_type;
                    metadata.default.as_ref().map_or_else(
                        || {
                            quote!(
                                <#field_type as ::godot_rs::engine::GodotIntegerValue>::
                                    __from_raw(
                                        <#field_type as ::godot_rs::engine::GodotIntegerValue>::
                                            PROPERTY_DEFAULT_RAW
                                    )
                            )
                        },
                        |value| quote!(#value),
                    )
                } else if matches!(
                    metadata.property_type,
                    Some(PropertyType::NodeRef | PropertyType::ResourceRef)
                ) {
                    let class = metadata
                        .property_object_class
                        .as_ref()
                        .expect("exported object class was validated");
                    let assertion = if metadata.property_type == Some(PropertyType::NodeRef) {
                        quote!(::godot_rs::script::assert_export_node::<#class>())
                    } else {
                        quote!(::godot_rs::script::assert_export_resource::<#class>())
                    };
                    quote!({
                        #assertion;
                        ::core::default::Default::default()
                    })
                } else if matches!(
                    metadata.property_type,
                    Some(PropertyType::String | PropertyType::StringName | PropertyType::NodePath)
                ) {
                    metadata.default.as_ref().map_or_else(
                        || quote!(::core::default::Default::default()),
                        |value| quote!(::core::convert::From::from(#value)),
                    )
                } else {
                    metadata
                        .default
                        .as_ref()
                        .map(|value| quote!(#value))
                        .unwrap_or_else(|| quote!(::core::default::Default::default()))
                };
                initializers.push(quote!(#name: #initializer));
                descriptors.push(field_descriptor(name, &generated_type, &metadata)?);
                if metadata.property_type.is_some() {
                    property_proxies.push(typed_script::property_api(
                        name,
                        &generated_type,
                        index,
                        metadata.property_read_only,
                    ));
                }
                if metadata.kind == FieldKind::Signal {
                    signal_proxies.push(typed_script::signal_api(name, &generated_type));
                }
                if metadata.property_type.is_some() || metadata.reload == ReloadPolicy::Persist {
                    let field_type = &generated_type;
                    let field_value = metadata.property_getter.as_ref().map_or_else(
                        || quote!(::core::clone::Clone::clone(&self.#name)),
                        |getter| quote!(self.#getter()),
                    );
                    field_getters.push(quote! {
                        #index => {
                            let field_value: #field_type = #field_value;
                            ::godot_rs::script::IntoMethodResult::write_result(
                                field_value,
                                output,
                            )
                        },
                    });
                    if !metadata.property_read_only {
                        let assignment = metadata.property_setter.as_ref().map_or_else(
                            || quote!(self.#name = decoded;),
                            |setter| quote!(self.#setter(decoded);),
                        );
                        field_setters.push(quote! {
                            #index => {
                                let Some(decoded) =
                                    <#field_type as ::godot_rs::script::FromAbiValue>::from_abi(value)
                                else {
                                    return ::godot_rs::abi::AbiCallResult::failure(
                                        ::godot_rs::abi::AbiStatus::InvalidArgument,
                                        "Host value does not match the generated Rust field type",
                                    );
                                };
                                #assignment
                                ::godot_rs::abi::AbiCallResult::OK
                            }
                        });
                    }
                } else if metadata.kind == FieldKind::Node {
                    let field_type = &generated_type;
                    field_setters.push(quote! {
                        #index => {
                            let Ok(decoded) =
                                <#field_type as ::godot_rs::engine::EngineReturn>::
                                    __from_engine_return(value)
                            else {
                                return ::godot_rs::abi::AbiCallResult::failure(
                                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                                    "resolved node does not match the generated Rust field type",
                                );
                            };
                            self.#name = decoded;
                            ::godot_rs::abi::AbiCallResult::OK
                        }
                    });
                }
            }
        }
        Fields::Unit => {}
        Fields::Unnamed(fields) => {
            return Err(Error::new_spanned(
                fields,
                "attachable scripts must use named fields or be a unit struct",
            ));
        }
    }

    let name = &item.ident;
    let api_name = typed_script::api_name(name);
    let api = typed_script::class_api(
        name,
        &item.vis,
        &base_type,
        &property_proxies,
        &signal_proxies,
    );
    let base_name = base_type.to_string();
    let tool = arguments.tool;
    let abstract_ = arguments.abstract_;
    let icon_path = arguments.icon.as_ref().map_or_else(
        || quote!(::core::option::Option::None),
        |path| {
            let path = LitStr::new(path, Span::call_site());
            quote!(::core::option::Option::Some(#path))
        },
    );
    let global_name = arguments.class_name.as_ref().map_or_else(
        || quote!(::core::option::Option::None),
        |name| {
            let name = LitStr::new(name, Span::call_site());
            quote!(::core::option::Option::Some(#name))
        },
    );
    let base_script = arguments.extends.as_ref().map_or_else(
        || quote!(::core::option::Option::None),
        |path| {
            let path = LitStr::new(path, Span::call_site());
            quote!(::core::option::Option::Some(#path))
        },
    );
    let task_field = syn::Field::parse_named.parse2(quote! {
        #[doc(hidden)]
        __godot_rs_tasks: ::std::cell::OnceCell<::godot_rs::task::TaskScope>
    })?;
    match &mut item.fields {
        Fields::Named(fields) => fields.named.push(task_field),
        Fields::Unit => {
            item.fields = Fields::Named(syn::parse_quote!({ #task_field }));
            item.semi_token = None;
        }
        Fields::Unnamed(_) => unreachable!("tuple fields were rejected"),
    }
    initializers.push(quote!(__godot_rs_tasks: ::std::cell::OnceCell::new()));
    let constructor = quote!(Self { #(#initializers),* });

    Ok(quote! {
        #item

        #api

        impl #name {
            /// Returns the Godot object that owns the current Rust script.
            #[must_use]
            pub fn base(&self) -> ::godot_rs::engine::Base<::godot_rs::engine::#base_type> {
                ::godot_rs::engine::Base::<::godot_rs::engine::#base_type>::__current()
            }
        }

        impl ::godot_rs::script::ScriptClass for #name {
            type Api = #api_name;

            const DESCRIPTOR: ::godot_rs::script::ScriptDescriptor = {
                const FIELDS: &'static [::godot_rs::script::FieldDescriptor] =
                    &[#(#descriptors),*];
                ::godot_rs::script::ScriptDescriptor {
                    name: stringify!(#name),
                    global_name: #global_name,
                    base_script: #base_script,
                    module_path: module_path!(),
                    base: #base_name,
                    tool: #tool,
                    abstract_: #abstract_,
                    icon_path: #icon_path,
                    fields: FIELDS,
                }
            };

            fn __godot_rs_new() -> Self {
                #constructor
            }
        }

        impl ::godot_rs::script::ScriptBase for #name {
            type Base = ::godot_rs::engine::#base_type;
        }

        impl ::godot_rs::script::ScriptFieldAccess for #name {
            unsafe fn __godot_rs_get_field(
                &self,
                field_index: u32,
                output: *mut ::godot_rs::abi::AbiValueV1,
            ) -> ::godot_rs::abi::AbiCallResult {
                match field_index {
                    #(#field_getters)*
                    _ => ::godot_rs::abi::AbiCallResult::failure(
                        ::godot_rs::abi::AbiStatus::Unsupported,
                        "field index is not an exported Rust property",
                    ),
                }
            }

            unsafe fn __godot_rs_set_field(
                &mut self,
                field_index: u32,
                value: ::godot_rs::abi::AbiValueV1,
            ) -> ::godot_rs::abi::AbiCallResult {
                match field_index {
                    #(#field_setters,)*
                    _ => ::godot_rs::abi::AbiCallResult::failure(
                        ::godot_rs::abi::AbiStatus::Unsupported,
                        "field index is not an exported Rust property",
                    ),
                }
            }
        }

        impl ::godot_rs::script::ScriptMethods for #name {}
    })
}

fn take_field_metadata(
    attributes: &mut Vec<Attribute>,
    field_type: &Type,
) -> syn::Result<FieldMetadata> {
    let mut metadata = FieldMetadata::default();
    let mut retained = Vec::with_capacity(attributes.len());
    let mut initializer_attribute = None;
    let mut property_attribute = None;
    for attribute in core::mem::take(attributes) {
        if attribute.path().is_ident("export") || attribute.path().is_ident("property") {
            let kind = if attribute.path().is_ident("export") {
                FieldKind::Export
            } else {
                FieldKind::Property
            };
            set_field_kind(&mut metadata, kind, &attribute)?;
            let entries = parse_meta_arguments(&attribute)?;
            let property_type = export_property_type(field_type, &attribute)?;
            metadata.property_type = Some(property_type);
            let schema = parse_export_schema(&entries, property_type)?;
            metadata.property_hint = schema.hint;
            metadata.property_hint_string = schema.hint_string;
            metadata.property_group = schema.group;
            let accessors = parse_property_accessors(&entries)?;
            metadata.property_getter = accessors.getter;
            metadata.property_setter = accessors.setter;
            metadata.property_read_only = accessors.read_only;
            if property_type == PropertyType::Array {
                let array = exported_array_metadata(field_type, &attribute)?;
                if let Some(array) = array {
                    metadata.property_hint = 23;
                    metadata.property_hint_string = array.hint_string;
                    metadata.typed_array_element = Some(array.element_name);
                    let element =
                        single_type_argument(field_type, "Array").expect("validated typed array");
                    metadata.typed_array_class = reflected_object_class(element);
                    metadata.property_hint_expression =
                        Some(array_hint_expression(element, &attribute)?);
                }
            }
            if matches!(
                property_type,
                PropertyType::NodeRef | PropertyType::ResourceRef
            ) {
                let object = exported_object_metadata(field_type, &attribute)?.expect(
                    "the exported property type was already identified as an object reference",
                );
                metadata.property_hint = if property_type == PropertyType::NodeRef {
                    34
                } else {
                    17
                };
                metadata.property_hint_string = object.class_name;
                metadata.property_object_class = Some(object.class_type);
                metadata.property_node_usage =
                    kind == FieldKind::Export && property_type == PropertyType::NodeRef;
            }
            merge_default(&mut metadata, find_default(&entries)?, &attribute)?;
            metadata.options = attribute_options(&attribute);
            property_attribute = Some(attribute);
        } else if attribute.path().is_ident("node") {
            set_field_kind(&mut metadata, FieldKind::Node, &attribute)?;
            let arguments = attribute.parse_args::<NodeArguments>()?;
            let optional =
                arguments.optional || single_type_argument(field_type, "Option").is_some();
            metadata.node_class = Some(validate_node_type(field_type, optional, &attribute)?);
            metadata.node_path = Some(arguments.path.value());
            metadata.node_optional = optional;
            metadata.options = attribute_options(&attribute);
        } else if attribute.path().is_ident("signal") {
            set_field_kind(&mut metadata, FieldKind::Signal, &attribute)?;
            metadata.signal_arguments = parse_signal_schema(&attribute, field_type)?;
            metadata.options = attribute_options(&attribute);
        } else if attribute.path().is_ident("reload") {
            let entries = parse_meta_arguments(&attribute)?;
            let (policy, default) = parse_reload_arguments(&entries)?;
            if metadata.reload != ReloadPolicy::Default && metadata.reload != policy {
                return Err(Error::new_spanned(
                    attribute,
                    "a field can only have one reload policy",
                ));
            }
            metadata.reload = policy;
            merge_default(&mut metadata, default, &attribute)?;
        } else if attribute.path().is_ident("init") {
            let entries = parse_meta_arguments(&attribute)?;
            if entries.len() != 1
                || !matches!(entries.first(), Some(Meta::NameValue(value)) if value.path.is_ident("default"))
            {
                return Err(Error::new_spanned(
                    attribute,
                    "field initialization uses `#[init(default = expression)]`",
                ));
            }
            merge_default(&mut metadata, find_default(&entries)?, &attribute)?;
            initializer_attribute = Some(attribute);
        } else {
            retained.push(attribute);
        }
    }
    if let Some(attribute) = initializer_attribute {
        if metadata.kind != FieldKind::Plain {
            return Err(Error::new_spanned(
                attribute,
                "`init` initializes plain Rust fields; use the property's `default` option, or let Godot bind nodes and signals",
            ));
        }
    }
    if let Some(attribute) = property_attribute {
        validate_property_default(&metadata, &attribute)?;
    }
    *attributes = retained;
    Ok(metadata)
}

fn validate_property_default(metadata: &FieldMetadata, source: &Attribute) -> syn::Result<()> {
    match metadata.property_type {
        Some(PropertyType::String | PropertyType::StringName | PropertyType::NodePath) => {
            validate_string_default(metadata.default.as_ref(), source)?;
        }
        Some(
            PropertyType::Array
            | PropertyType::Dictionary
            | PropertyType::PackedByteArray
            | PropertyType::PackedInt32Array
            | PropertyType::PackedInt64Array
            | PropertyType::PackedFloat32Array
            | PropertyType::PackedFloat64Array
            | PropertyType::PackedStringArray
            | PropertyType::PackedVector2Array
            | PropertyType::PackedVector3Array
            | PropertyType::PackedColorArray
            | PropertyType::PackedVector4Array,
        ) if metadata.default.is_some() => {
            return Err(Error::new_spanned(
                source,
                "container exports currently use their empty Godot value as the default; remove `default = ...`",
            ));
        }
        Some(PropertyType::NodeRef | PropertyType::ResourceRef) if metadata.default.is_some() => {
            return Err(Error::new_spanned(
                source,
                "exported Godot object references use `None` as their default; remove `default = ...`",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn set_field_kind(
    metadata: &mut FieldMetadata,
    kind: FieldKind,
    attribute: &Attribute,
) -> syn::Result<()> {
    if metadata.kind != FieldKind::Plain {
        return Err(Error::new_spanned(
            attribute,
            "a field can only be one of `property`, `export`, `node`, or `signal`",
        ));
    }
    metadata.kind = kind;
    Ok(())
}

fn merge_default(
    metadata: &mut FieldMetadata,
    value: Option<Expr>,
    source: &Attribute,
) -> syn::Result<()> {
    if let Some(value) = value {
        if metadata.default.is_some() {
            return Err(Error::new_spanned(
                source,
                "a field can only declare one generated default value",
            ));
        }
        metadata.default = Some(value);
    }
    Ok(())
}

fn parse_meta_arguments(attribute: &Attribute) -> syn::Result<Punctuated<Meta, Token![,]>> {
    match &attribute.meta {
        Meta::Path(_) => Ok(Punctuated::new()),
        Meta::List(_) => attribute.parse_args_with(Punctuated::parse_terminated),
        Meta::NameValue(_) => Err(Error::new_spanned(
            attribute,
            "use parentheses for field attribute arguments",
        )),
    }
}

struct ExportPropertySchema {
    hint: u32,
    hint_string: String,
    group: Option<String>,
}

#[derive(Default)]
struct PropertyAccessors {
    getter: Option<syn::Ident>,
    setter: Option<syn::Ident>,
    read_only: bool,
}

fn parse_property_accessors(
    entries: &Punctuated<Meta, Token![,]>,
) -> syn::Result<PropertyAccessors> {
    let mut result = PropertyAccessors::default();
    for entry in entries {
        match entry {
            Meta::NameValue(value) if value.path.is_ident("get") => {
                result.getter = Some(property_accessor_name(value, result.getter.is_some())?);
            }
            Meta::NameValue(value) if value.path.is_ident("set") => {
                if result.read_only {
                    return Err(Error::new_spanned(
                        value,
                        "a read-only property cannot declare a setter",
                    ));
                }
                result.setter = Some(property_accessor_name(value, result.setter.is_some())?);
            }
            Meta::Path(path) if path.is_ident("read_only") => {
                if result.read_only {
                    return Err(Error::new_spanned(path, "duplicate `read_only` option"));
                }
                if result.setter.is_some() {
                    return Err(Error::new_spanned(
                        path,
                        "a property with a setter cannot be read-only",
                    ));
                }
                result.read_only = true;
            }
            _ => {}
        }
    }
    Ok(result)
}

fn property_accessor_name(value: &syn::MetaNameValue, duplicate: bool) -> syn::Result<syn::Ident> {
    if duplicate {
        return Err(Error::new_spanned(value, "duplicate property accessor"));
    }
    let Expr::Path(path) = &value.value else {
        return Err(Error::new_spanned(
            &value.value,
            "a property accessor must be a Rust method name",
        ));
    };
    if path.qself.is_some() || path.path.segments.len() != 1 {
        return Err(Error::new_spanned(
            path,
            "a property accessor must be one Rust method name",
        ));
    }
    Ok(path.path.segments[0].ident.clone())
}

fn export_property_type(field_type: &Type, source: &Attribute) -> syn::Result<PropertyType> {
    match outer_type_name(field_type).as_deref() {
        Some("bool") => Ok(PropertyType::Bool),
        Some("i32" | "i64") => Ok(PropertyType::Int),
        Some("f32" | "f64") => Ok(PropertyType::Float),
        Some("String") => Ok(PropertyType::String),
        Some("StringName") => Ok(PropertyType::StringName),
        Some("NodePath") => Ok(PropertyType::NodePath),
        Some("Vector2") => Ok(PropertyType::Vector2),
        Some("Vector2i") => Ok(PropertyType::Vector2i),
        Some("Vector3") => Ok(PropertyType::Vector3),
        Some("Vector3i") => Ok(PropertyType::Vector3i),
        Some("Vector4") => Ok(PropertyType::Vector4),
        Some("Vector4i") => Ok(PropertyType::Vector4i),
        Some("Rect2") => Ok(PropertyType::Rect2),
        Some("Rect2i") => Ok(PropertyType::Rect2i),
        Some("Quaternion") => Ok(PropertyType::Quaternion),
        Some("Plane") => Ok(PropertyType::Plane),
        Some("Transform2D") => Ok(PropertyType::Transform2D),
        Some("Aabb") => Ok(PropertyType::Aabb),
        Some("Basis") => Ok(PropertyType::Basis),
        Some("Transform3D") => Ok(PropertyType::Transform3D),
        Some("Projection") => Ok(PropertyType::Projection),
        Some("Color") => Ok(PropertyType::Color),
        Some("Array") => Ok(PropertyType::Array),
        Some("Dictionary") => Ok(PropertyType::Dictionary),
        Some("PackedByteArray") => Ok(PropertyType::PackedByteArray),
        Some("PackedInt32Array") => Ok(PropertyType::PackedInt32Array),
        Some("PackedInt64Array") => Ok(PropertyType::PackedInt64Array),
        Some("PackedFloat32Array") => Ok(PropertyType::PackedFloat32Array),
        Some("PackedFloat64Array") => Ok(PropertyType::PackedFloat64Array),
        Some("PackedStringArray") => Ok(PropertyType::PackedStringArray),
        Some("PackedVector2Array") => Ok(PropertyType::PackedVector2Array),
        Some("PackedVector3Array") => Ok(PropertyType::PackedVector3Array),
        Some("PackedColorArray") => Ok(PropertyType::PackedColorArray),
        Some("PackedVector4Array") => Ok(PropertyType::PackedVector4Array),
        _ if exported_object_metadata(field_type, source)?.is_some() => {
            let object = exported_object_metadata(field_type, source)?
                .expect("exported object metadata was just recognized");
            Ok(object.property_type)
        }
        Some("GodotRef" | "NodeRef" | "ObjectRef") => Err(Error::new_spanned(
            source,
            "exported Godot object fields must be nullable: use `Option<GodotRef<ResourceClass>>` or `Option<NodeRef<NodeClass>>`",
        )),
        _ if matches!(field_type, Type::Path(_)) && !has_type_arguments(field_type) => {
            Ok(PropertyType::GodotInteger)
        }
        _ => Err(Error::new_spanned(
            source,
            "`#[export]` supports scalars, Godot text and math values, packed arrays, Array<T>, Dictionary, Option<NodeRef<T>>, and Option<GodotRef<T>> fields",
        )),
    }
}

struct ExportedObjectMetadata {
    property_type: PropertyType,
    class_type: Type,
    class_name: String,
}

fn exported_object_metadata(
    field_type: &Type,
    source: &Attribute,
) -> syn::Result<Option<ExportedObjectMetadata>> {
    let Some(inner) = single_type_argument(field_type, "Option") else {
        return Ok(None);
    };
    let property_type = if single_type_argument(inner, "NodeRef").is_some() {
        PropertyType::NodeRef
    } else if single_type_argument(inner, "GodotRef").is_some() {
        PropertyType::ResourceRef
    } else if single_type_argument(inner, "ObjectRef").is_some() {
        return Err(Error::new_spanned(
            source,
            "use `Option<NodeRef<NodeClass>>` for scene nodes or `Option<GodotRef<ResourceClass>>` for resources",
        ));
    } else {
        return Ok(None);
    };
    let wrapper = if property_type == PropertyType::NodeRef {
        "NodeRef"
    } else {
        "GodotRef"
    };
    let class_type = single_type_argument(inner, wrapper)
        .expect("the exported object wrapper was recognized")
        .clone();
    let class_name = simple_type_name(&class_type, source)?;
    Ok(Some(ExportedObjectMetadata {
        property_type,
        class_type,
        class_name,
    }))
}

struct ExportedArrayMetadata {
    hint_string: String,
    element_name: String,
}

fn exported_array_metadata(
    field_type: &Type,
    source: &Attribute,
) -> syn::Result<Option<ExportedArrayMetadata>> {
    let Type::Path(path) = field_type else {
        return Err(Error::new_spanned(
            source,
            "an exported Godot Array must use `Array` or `Array<T>`",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            source,
            "an exported Godot Array must use `Array` or `Array<T>`",
        ));
    };
    match &segment.arguments {
        PathArguments::None => Ok(None),
        PathArguments::AngleBracketed(arguments) if arguments.args.len() == 1 => {
            let Some(GenericArgument::Type(element)) = arguments.args.first() else {
                return Err(Error::new_spanned(
                    arguments,
                    "an exported Godot Array requires one element type",
                ));
            };
            let (hint_string, element_name) = array_element_metadata(element, source)?;
            Ok(Some(ExportedArrayMetadata {
                hint_string,
                element_name,
            }))
        }
        arguments => Err(Error::new_spanned(
            arguments,
            "an exported Godot Array requires exactly one element type",
        )),
    }
}

fn array_element_metadata(element: &Type, source: &Attribute) -> syn::Result<(String, String)> {
    if let Some(inner) = single_type_argument(element, "GodotRef") {
        let class = simple_type_name(inner, source)?;
        return Ok((format!("24/17:{class}"), class));
    }
    if let Some(inner) = single_type_argument(element, "ObjectRef")
        .or_else(|| single_type_argument(element, "NodeRef"))
    {
        let class = simple_type_name(inner, source)?;
        return Ok((format!("24/34:{class}"), class));
    }
    if outer_type_name(element).as_deref() == Some("Array") {
        let nested = exported_array_metadata(element, source)?;
        let nested_hint = nested
            .map(|metadata| metadata.hint_string)
            .unwrap_or_else(|| "0:".to_owned());
        return Ok((format!("28:{nested_hint}"), "Array".to_owned()));
    }
    let Some(name) = outer_type_name(element) else {
        return Err(Error::new_spanned(
            source,
            "an exported Array element must be a supported Godot value type",
        ));
    };
    let (variant_type, godot_name) = match name.as_str() {
        "Variant" => (0, "Variant"),
        "bool" => (1, "bool"),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "char" => (2, "int"),
        "f32" | "f64" => (3, "float"),
        "String" => (4, "String"),
        "Vector2" => (5, "Vector2"),
        "Vector2i" => (6, "Vector2i"),
        "Rect2" => (7, "Rect2"),
        "Rect2i" => (8, "Rect2i"),
        "Vector3" => (9, "Vector3"),
        "Vector3i" => (10, "Vector3i"),
        "Transform2D" => (11, "Transform2D"),
        "Vector4" => (12, "Vector4"),
        "Vector4i" => (13, "Vector4i"),
        "Plane" => (14, "Plane"),
        "Quaternion" => (15, "Quaternion"),
        "Aabb" => (16, "AABB"),
        "Basis" => (17, "Basis"),
        "Transform3D" => (18, "Transform3D"),
        "Projection" => (19, "Projection"),
        "Color" => (20, "Color"),
        "StringName" => (21, "StringName"),
        "NodePath" => (22, "NodePath"),
        "Rid" => (23, "RID"),
        "Callable" => (25, "Callable"),
        "Signal" => (26, "Signal"),
        "Dictionary" => (27, "Dictionary"),
        "PackedByteArray" => (29, "PackedByteArray"),
        "PackedInt32Array" => (30, "PackedInt32Array"),
        "PackedInt64Array" => (31, "PackedInt64Array"),
        "PackedFloat32Array" => (32, "PackedFloat32Array"),
        "PackedFloat64Array" => (33, "PackedFloat64Array"),
        "PackedStringArray" => (34, "PackedStringArray"),
        "PackedVector2Array" => (35, "PackedVector2Array"),
        "PackedVector3Array" => (36, "PackedVector3Array"),
        "PackedColorArray" => (37, "PackedColorArray"),
        "PackedVector4Array" => (38, "PackedVector4Array"),
        // Generated Godot enums and bitfields implement `VariantConvert` as
        // integers; the Rust compiler still rejects unrelated element types.
        _ => (2, "int"),
    };
    Ok((format!("{variant_type}:"), godot_name.to_owned()))
}

fn array_hint_expression(element: &Type, source: &Attribute) -> syn::Result<TokenStream2> {
    let parts = if let Some(class) = reflected_object_class(element) {
        let prefix = if single_type_argument(element, "GodotRef").is_some() {
            "24/17:"
        } else {
            "24/34:"
        };
        vec![
            quote!(#prefix),
            quote!(<#class as ::godot_rs::engine::GodotClass>::CLASS_NAME),
        ]
    } else if let Some(nested) = single_type_argument(element, "Array") {
        let nested = array_hint_expression(nested, source)?;
        vec![quote!("28:"), nested]
    } else {
        let (hint, _) = array_element_metadata(element, source)?;
        return Ok(quote!(#hint));
    };
    Ok(quote!({
        const PARTS: &[&str] = &[#(#parts),*];
        const LENGTH: usize = {
            let mut length = 0;
            let mut index = 0;
            while index < PARTS.len() {
                length += PARTS[index].len();
                index += 1;
            }
            length
        };
        const BYTES: [u8; LENGTH] = {
            let mut output = [0; LENGTH];
            let mut offset = 0;
            let mut part = 0;
            while part < PARTS.len() {
                let bytes = PARTS[part].as_bytes();
                let mut index = 0;
                while index < bytes.len() {
                    output[offset] = bytes[index];
                    offset += 1;
                    index += 1;
                }
                part += 1;
            }
            output
        };
        match ::core::str::from_utf8(&BYTES) {
            ::core::result::Result::Ok(value) => value,
            ::core::result::Result::Err(_) => panic!("Godot property hints are UTF-8"),
        }
    }))
}

fn simple_type_name(type_: &Type, source: &Attribute) -> syn::Result<String> {
    let Type::Path(path) = type_ else {
        return Err(Error::new_spanned(
            source,
            "an exported Array object element must use one Godot class or Rust script type",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            source,
            "an exported Array object element must use one Godot class or Rust script type",
        ));
    };
    if !matches!(segment.arguments, PathArguments::None) {
        return Err(Error::new_spanned(
            source,
            "an exported Array object element must use one Godot class or Rust script type",
        ));
    }
    Ok(segment.ident.to_string())
}

fn parse_export_schema(
    entries: &Punctuated<Meta, Token![,]>,
    type_: PropertyType,
) -> syn::Result<ExportPropertySchema> {
    let mut hint = 0;
    let mut hint_string = String::new();
    let mut group = None;
    for entry in entries {
        match entry {
            Meta::NameValue(value) if value.path.is_ident("default") => {}
            Meta::NameValue(value) if value.path.is_ident("get") || value.path.is_ident("set") => {}
            Meta::Path(path) if path.is_ident("read_only") => {}
            Meta::NameValue(value) if value.path.is_ident("group") => {
                if group.is_some() {
                    return Err(Error::new_spanned(value, "duplicate export group"));
                }
                let Expr::Lit(expression) = &value.value else {
                    return Err(Error::new_spanned(
                        &value.value,
                        "export group must be a string literal",
                    ));
                };
                let syn::Lit::Str(name) = &expression.lit else {
                    return Err(Error::new_spanned(
                        &expression.lit,
                        "export group must be a string literal",
                    ));
                };
                if name.value().is_empty() {
                    return Err(Error::new_spanned(name, "export group cannot be empty"));
                }
                group = Some(name.value());
            }
            Meta::List(list) if list.path.is_ident("range") => {
                if !matches!(type_, PropertyType::Int | PropertyType::Float) {
                    return Err(Error::new_spanned(
                        list,
                        "`range(...)` requires an integer or float field",
                    ));
                }
                set_property_hint(&mut hint, 1, list)?;
                hint_string = parse_range_hint(list)?;
            }
            Meta::Path(path) if path.is_ident("flags") => {
                if type_ != PropertyType::Int {
                    return Err(Error::new_spanned(
                        path,
                        "`flags` requires an integer field",
                    ));
                }
                set_property_hint(&mut hint, 6, path)?;
            }
            Meta::Path(path) if path.is_ident("multiline") => {
                if type_ != PropertyType::String {
                    return Err(Error::new_spanned(
                        path,
                        "`multiline` requires a String field",
                    ));
                }
                set_property_hint(&mut hint, 18, path)?;
            }
            Meta::List(list) if list.path.is_ident("file") => {
                if type_ != PropertyType::String {
                    return Err(Error::new_spanned(
                        list,
                        "`file(...)` requires a String field",
                    ));
                }
                set_property_hint(&mut hint, 13, list)?;
                hint_string = parse_file_hint(list)?;
            }
            Meta::Path(path) if path.is_ident("no_alpha") => {
                if type_ != PropertyType::Color {
                    return Err(Error::new_spanned(
                        path,
                        "`no_alpha` requires a Color field",
                    ));
                }
                set_property_hint(&mut hint, 21, path)?;
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "unsupported property option; use `default`, `get`, `set`, `read_only`, `group`, `range`, `flags`, `file`, `multiline`, or `no_alpha`",
                ));
            }
        }
    }
    Ok(ExportPropertySchema {
        hint,
        hint_string,
        group,
    })
}

fn parse_file_hint(list: &syn::MetaList) -> syn::Result<String> {
    let filters = list.parse_args_with(Punctuated::<LitStr, Token![,]>::parse_terminated)?;
    match filters.len() {
        0 => Ok(String::new()),
        1 => Ok(filters
            .first()
            .expect("one file filter was counted")
            .value()),
        _ => Err(Error::new_spanned(
            list,
            "`file(...)` accepts at most one filter string",
        )),
    }
}

fn validate_string_default(default: Option<&Expr>, source: &Attribute) -> syn::Result<()> {
    let Some(default) = default else {
        return Ok(());
    };
    if matches!(default, Expr::Lit(expression) if matches!(expression.lit, syn::Lit::Str(_))) {
        return Ok(());
    }
    Err(Error::new_spanned(
        source,
        "a String export default must be a string literal",
    ))
}

fn set_property_hint(current: &mut u32, next: u32, source: &impl ToTokens) -> syn::Result<()> {
    if *current != 0 {
        return Err(Error::new_spanned(
            source,
            "an export field can only declare one Inspector hint",
        ));
    }
    *current = next;
    Ok(())
}

fn parse_range_hint(list: &syn::MetaList) -> syn::Result<String> {
    let entries = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let mut minimum = None;
    let mut maximum = None;
    let mut step = None;
    let mut modifiers = Vec::new();
    for entry in entries {
        match entry {
            Meta::NameValue(value) if value.path.is_ident("min") => {
                set_hint_value(
                    &mut minimum,
                    value.value.to_token_stream().to_string(),
                    &value,
                )?;
            }
            Meta::NameValue(value) if value.path.is_ident("max") => {
                set_hint_value(
                    &mut maximum,
                    value.value.to_token_stream().to_string(),
                    &value,
                )?;
            }
            Meta::NameValue(value) if value.path.is_ident("step") => {
                set_hint_value(&mut step, value.value.to_token_stream().to_string(), &value)?;
            }
            Meta::NameValue(value) if value.path.is_ident("suffix") => {
                let Expr::Lit(expression) = &value.value else {
                    return Err(Error::new_spanned(
                        &value.value,
                        "range suffix must be a string literal",
                    ));
                };
                let syn::Lit::Str(suffix) = &expression.lit else {
                    return Err(Error::new_spanned(
                        &expression.lit,
                        "range suffix must be a string literal",
                    ));
                };
                modifiers.push(format!("suffix:{}", suffix.value()));
            }
            Meta::Path(path)
                if path.is_ident("or_greater")
                    || path.is_ident("or_less")
                    || path.is_ident("hide_slider")
                    || path.is_ident("radians_as_degrees")
                    || path.is_ident("degrees")
                    || path.is_ident("exp") =>
            {
                modifiers.push(
                    path.get_ident()
                        .expect("validated single-segment range flag")
                        .to_string(),
                );
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "unsupported range option; use `min`, `max`, `step`, `suffix`, or a Godot range flag",
                ));
            }
        }
    }
    let minimum = minimum.ok_or_else(|| Error::new_spanned(list, "`range(...)` requires `min`"))?;
    let maximum = maximum.ok_or_else(|| Error::new_spanned(list, "`range(...)` requires `max`"))?;
    let step = step.unwrap_or_else(|| "0.01".into());
    let mut parts = vec![minimum, maximum, step];
    parts.extend(modifiers);
    Ok(parts.join(","))
}

fn set_hint_value(
    current: &mut Option<String>,
    value: String,
    source: &impl ToTokens,
) -> syn::Result<()> {
    if current.replace(value).is_some() {
        return Err(Error::new_spanned(source, "duplicate range option"));
    }
    Ok(())
}

fn find_default(entries: &Punctuated<Meta, Token![,]>) -> syn::Result<Option<Expr>> {
    let mut default = None;
    for entry in entries {
        if let Meta::NameValue(value) = entry {
            if value.path.is_ident("default") && default.replace(value.value.clone()).is_some() {
                return Err(Error::new_spanned(value, "duplicate `default` value"));
            }
        }
    }
    Ok(default)
}

struct NodeArguments {
    path: LitStr,
    optional: bool,
}

impl Parse for NodeArguments {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let path = input.parse::<LitStr>()?;
        let mut optional = false;
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let option = input.parse::<Path>()?;
            if !option.is_ident("optional") {
                return Err(Error::new_spanned(
                    option,
                    "the only supported node flag is `optional`",
                ));
            }
            if optional {
                return Err(Error::new_spanned(option, "duplicate `optional` flag"));
            }
            optional = true;
        }
        if path.value().is_empty() {
            return Err(Error::new_spanned(path, "node path cannot be empty"));
        }
        Ok(Self { path, optional })
    }
}

fn validate_node_type(field_type: &Type, optional: bool, source: &Attribute) -> syn::Result<Type> {
    let node_ref = if optional {
        single_type_argument(field_type, "Option").ok_or_else(|| {
            Error::new_spanned(
                source,
                "an optional node field must use `Option<NodeRef<T>>`",
            )
        })?
    } else {
        field_type
    };
    let target = single_type_argument(node_ref, "NodeRef").ok_or_else(|| {
        Error::new_spanned(
            source,
            if optional {
                "an optional node field must use `Option<NodeRef<T>>`"
            } else {
                "a required node field must use `NodeRef<T>`"
            },
        )
    })?;
    let Type::Path(path) = target else {
        return Err(Error::new_spanned(
            source,
            "a node target must be one Godot class or Rust script type",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            source,
            "a node target must be one Godot class or Rust script type",
        ));
    };
    if !matches!(segment.arguments, PathArguments::None) {
        return Err(Error::new_spanned(
            source,
            "a node target must be one Godot class or Rust script type",
        ));
    }
    Ok(target.clone())
}

fn single_type_argument<'a>(field_type: &'a Type, expected: &str) -> Option<&'a Type> {
    let Type::Path(path) = field_type else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != expected {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    if arguments.args.len() != 1 {
        return None;
    }
    match arguments.args.first()? {
        GenericArgument::Type(type_) => Some(type_),
        _ => None,
    }
}

fn parse_signal_schema(
    attribute: &Attribute,
    field_type: &Type,
) -> syn::Result<Vec<SignalArgumentMetadata>> {
    validate_outer_type(field_type, "Signal", attribute)?;
    let entries = parse_meta_arguments(attribute)?;
    if let Some(Type::BareFn(signature)) = single_type_argument(field_type, "Signal") {
        if !entries.is_empty() {
            return Err(Error::new_spanned(
                attribute,
                "function-style signals declare argument names in `Signal<fn(name: Type, ...)>`; remove `args(...)`",
            ));
        }
        if signature.unsafety.is_some()
            || signature.abi.is_some()
            || signature.variadic.is_some()
            || signature.lifetimes.is_some()
            || !matches!(&signature.output, ReturnType::Default)
                && !matches!(&signature.output, ReturnType::Type(_, ty) if matches!(ty.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty()))
        {
            return Err(Error::new_spanned(
                signature,
                "signals use a safe Rust function signature with no return value, such as `Signal<fn(amount: i64)>`",
            ));
        }
        if signature.inputs.len() > 8 {
            return Err(Error::new_spanned(
                signature,
                "signals currently support at most 8 arguments",
            ));
        }
        let mut names = std::collections::HashSet::new();
        return signature
            .inputs
            .iter()
            .map(|argument| {
                let Some((name, _)) = &argument.name else {
                    return Err(Error::new_spanned(
                        argument,
                        "name each signal argument, such as `Signal<fn(amount: i64)>`",
                    ));
                };
                if name == "_" || !names.insert(name.to_string()) {
                    return Err(Error::new_spanned(
                        name,
                        "signal argument names must be unique and cannot be `_`",
                    ));
                }
                abi_value_type(&argument.ty)?;
                Ok(SignalArgumentMetadata {
                    name: name.clone(),
                    type_: argument.ty.clone(),
                })
            })
            .collect();
    }
    let mut names = None;
    for entry in entries {
        match entry {
            Meta::List(list) if list.path.is_ident("args") => {
                if names.is_some() {
                    return Err(Error::new_spanned(list, "duplicate signal `args(...)`"));
                }
                names = Some(
                    list.parse_args_with(Punctuated::<syn::Ident, Token![,]>::parse_terminated)?
                        .into_iter()
                        .collect::<Vec<_>>(),
                );
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "signal only supports `args(name, ...)`",
                ));
            }
        }
    }
    let names = names.unwrap_or_default();
    let argument_types = signal_argument_types(field_type)?;
    if argument_types.len() > 8 {
        return Err(Error::new_spanned(
            attribute,
            "signals currently support at most 8 arguments",
        ));
    }
    if names.len() != argument_types.len() {
        return Err(Error::new_spanned(
            attribute,
            format!(
                "signal declares {} argument name(s) but its tuple contains {} type(s)",
                names.len(),
                argument_types.len()
            ),
        ));
    }
    let mut unique_names = std::collections::HashSet::new();
    for name in &names {
        if name == "_" || !unique_names.insert(name.to_string()) {
            return Err(Error::new_spanned(
                name,
                "signal argument names must be unique and cannot be `_`",
            ));
        }
    }
    names
        .into_iter()
        .zip(argument_types)
        .map(|(name, type_)| {
            abi_value_type(&type_)?;
            Ok(SignalArgumentMetadata { name, type_ })
        })
        .collect()
}

fn signal_argument_types(field_type: &Type) -> syn::Result<Vec<Type>> {
    let Type::Path(path) = field_type else {
        return Err(Error::new_spanned(
            field_type,
            "Signal arguments must use a tuple such as `Signal<(i32, i32)>`",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            field_type,
            "Signal has no argument type",
        ));
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new_spanned(
            field_type,
            "Signal arguments must use a tuple such as `Signal<(i32, i32)>`",
        ));
    };
    let Some(GenericArgument::Type(Type::Tuple(tuple))) = arguments.args.first() else {
        return Err(Error::new_spanned(
            arguments,
            "Signal arguments must use a tuple such as `Signal<(i32, i32)>`",
        ));
    };
    if arguments.args.len() != 1 {
        return Err(Error::new_spanned(
            arguments,
            "Signal requires exactly one tuple type argument",
        ));
    }
    Ok(tuple.elems.iter().cloned().collect())
}

fn validate_outer_type(field_type: &Type, expected: &str, source: &Attribute) -> syn::Result<()> {
    if outer_type_name(field_type).as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(Error::new_spanned(
            source,
            format!("this field must use `{expected}<...>`"),
        ))
    }
}

fn outer_type_name(field_type: &Type) -> Option<String> {
    let Type::Path(path) = field_type else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn parse_reload_arguments(
    entries: &Punctuated<Meta, Token![,]>,
) -> syn::Result<(ReloadPolicy, Option<Expr>)> {
    let mut policy = ReloadPolicy::Default;
    let mut default = None;
    for entry in entries {
        match entry {
            Meta::Path(path) if path.is_ident("persist") => {
                set_reload_policy(&mut policy, ReloadPolicy::Persist, path)?;
            }
            Meta::Path(path) if path.is_ident("skip") => {
                set_reload_policy(&mut policy, ReloadPolicy::Skip, path)?;
            }
            Meta::NameValue(value) if value.path.is_ident("default") => {
                if default.replace(value.value.clone()).is_some() {
                    return Err(Error::new_spanned(value, "duplicate `default` value"));
                }
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "reload supports `persist`, `skip`, and `default = expression`",
                ));
            }
        }
    }
    Ok((policy, default))
}

fn set_reload_policy(
    current: &mut ReloadPolicy,
    next: ReloadPolicy,
    source: &Path,
) -> syn::Result<()> {
    if *current != ReloadPolicy::Default {
        return Err(Error::new_spanned(
            source,
            "choose either `persist` or `skip`, not both",
        ));
    }
    *current = next;
    Ok(())
}

fn field_descriptor(
    name: &syn::Ident,
    field_type: &Type,
    metadata: &FieldMetadata,
) -> syn::Result<TokenStream2> {
    let rust_type = token_string(field_type);
    let options = LitStr::new(&metadata.options, Span::call_site());
    let default = metadata.default.as_ref().map(|value| {
        let value = token_string(value);
        quote!(::core::option::Option::Some(#value))
    });
    let default = default.unwrap_or_else(|| quote!(::core::option::Option::None));
    let kind = match metadata.kind {
        FieldKind::Plain => quote!(::godot_rs::script::FieldKind::Plain),
        FieldKind::Export => quote!(::godot_rs::script::FieldKind::Export),
        FieldKind::Property => quote!(::godot_rs::script::FieldKind::Property),
        FieldKind::Node => quote!(::godot_rs::script::FieldKind::Node),
        FieldKind::Signal => quote!(::godot_rs::script::FieldKind::Signal),
    };
    let reload = match metadata.reload {
        ReloadPolicy::Default => quote!(::godot_rs::script::ReloadPolicy::Default),
        ReloadPolicy::Persist => quote!(::godot_rs::script::ReloadPolicy::Persist),
        ReloadPolicy::Skip => quote!(::godot_rs::script::ReloadPolicy::Skip),
    };
    let reload_value_type =
        if metadata.reload == ReloadPolicy::Persist && metadata.property_type.is_none() {
            let type_ = abi_value_type(field_type)?;
            quote!(::core::option::Option::Some(#type_))
        } else {
            quote!(::core::option::Option::None)
        };
    let property = metadata.property_type.map(|type_| {
        let abi_type = match type_ {
            PropertyType::Bool => quote!(::godot_rs::abi::AbiPropertyType::BOOL),
            PropertyType::Int => quote!(::godot_rs::abi::AbiPropertyType::INT),
            PropertyType::Float => quote!(::godot_rs::abi::AbiPropertyType::FLOAT),
            PropertyType::String => quote!(::godot_rs::abi::AbiPropertyType::STRING),
            PropertyType::StringName => quote!(::godot_rs::abi::AbiPropertyType::STRING_NAME),
            PropertyType::NodePath => quote!(::godot_rs::abi::AbiPropertyType::NODE_PATH),
            PropertyType::Vector2 => quote!(::godot_rs::abi::AbiPropertyType::VECTOR2),
            PropertyType::Vector2i => quote!(::godot_rs::abi::AbiPropertyType::VECTOR2I),
            PropertyType::Vector3 => quote!(::godot_rs::abi::AbiPropertyType::VECTOR3),
            PropertyType::Vector3i => quote!(::godot_rs::abi::AbiPropertyType::VECTOR3I),
            PropertyType::Vector4 => quote!(::godot_rs::abi::AbiPropertyType::VECTOR4),
            PropertyType::Vector4i => quote!(::godot_rs::abi::AbiPropertyType::VECTOR4I),
            PropertyType::Rect2 => quote!(::godot_rs::abi::AbiPropertyType::RECT2),
            PropertyType::Rect2i => quote!(::godot_rs::abi::AbiPropertyType::RECT2I),
            PropertyType::Quaternion => quote!(::godot_rs::abi::AbiPropertyType::QUATERNION),
            PropertyType::Plane => quote!(::godot_rs::abi::AbiPropertyType::PLANE),
            PropertyType::Transform2D => quote!(::godot_rs::abi::AbiPropertyType::TRANSFORM2D),
            PropertyType::Aabb => quote!(::godot_rs::abi::AbiPropertyType::AABB),
            PropertyType::Basis => quote!(::godot_rs::abi::AbiPropertyType::BASIS),
            PropertyType::Transform3D => quote!(::godot_rs::abi::AbiPropertyType::TRANSFORM3D),
            PropertyType::Projection => quote!(::godot_rs::abi::AbiPropertyType::PROJECTION),
            PropertyType::Color => quote!(::godot_rs::abi::AbiPropertyType::COLOR),
            PropertyType::Array => quote!(::godot_rs::abi::AbiPropertyType::ARRAY),
            PropertyType::Dictionary => quote!(::godot_rs::abi::AbiPropertyType::DICTIONARY),
            PropertyType::PackedByteArray => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_BYTE_ARRAY)
            }
            PropertyType::PackedInt32Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_INT32_ARRAY)
            }
            PropertyType::PackedInt64Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_INT64_ARRAY)
            }
            PropertyType::PackedFloat32Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_FLOAT32_ARRAY)
            }
            PropertyType::PackedFloat64Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_FLOAT64_ARRAY)
            }
            PropertyType::PackedStringArray => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_STRING_ARRAY)
            }
            PropertyType::PackedVector2Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_VECTOR2_ARRAY)
            }
            PropertyType::PackedVector3Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_VECTOR3_ARRAY)
            }
            PropertyType::PackedColorArray => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_COLOR_ARRAY)
            }
            PropertyType::PackedVector4Array => {
                quote!(::godot_rs::abi::AbiPropertyType::PACKED_VECTOR4_ARRAY)
            }
            PropertyType::NodeRef | PropertyType::ResourceRef => {
                quote!(::godot_rs::abi::AbiPropertyType::OBJECT)
            }
            PropertyType::GodotInteger => quote!(::godot_rs::abi::AbiPropertyType::INT),
        };
        let default_value = match (type_, metadata.default.as_ref()) {
            (PropertyType::Bool, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_bool(#value)
                    )
                ))
            }
            (PropertyType::Bool, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_bool(false)
                    )
                ))
            }
            (PropertyType::Int, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_i64((#value) as i64)
                    )
                ))
            }
            (PropertyType::Int, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_i64(0)
                    )
                ))
            }
            (PropertyType::Float, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_f64((#value) as f64)
                    )
                ))
            }
            (PropertyType::Float, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_f64(0.0)
                    )
                ))
            }
            (PropertyType::String, Some(Expr::Lit(expression))) => {
                let syn::Lit::Str(value) = &expression.lit else {
                    unreachable!("String defaults were validated as literals")
                };
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::String(#value)
                ))
            }
            (PropertyType::String, Some(_)) => {
                unreachable!("String defaults were validated as literals")
            }
            (PropertyType::String, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::String("")
                ))
            }
            (PropertyType::StringName, Some(Expr::Lit(expression))) => {
                let syn::Lit::Str(value) = &expression.lit else {
                    unreachable!("StringName defaults were validated as literals")
                };
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::StringName(#value)
                ))
            }
            (PropertyType::StringName, Some(_)) => {
                unreachable!("StringName defaults were validated as literals")
            }
            (PropertyType::StringName, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::StringName("")
                ))
            }
            (PropertyType::NodePath, Some(Expr::Lit(expression))) => {
                let syn::Lit::Str(value) = &expression.lit else {
                    unreachable!("NodePath defaults were validated as literals")
                };
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::NodePath(#value)
                ))
            }
            (PropertyType::NodePath, Some(_)) => {
                unreachable!("NodePath defaults were validated as literals")
            }
            (PropertyType::NodePath, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::NodePath("")
                ))
            }
            (PropertyType::Vector2, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector2 = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector2(value.x, value.y)
                    })
                ))
            }
            (PropertyType::Vector2, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector2(0.0, 0.0)
                    )
                ))
            }
            (PropertyType::Vector2i, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector2i = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector2i(value.x, value.y)
                    })
                ))
            }
            (PropertyType::Vector2i, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector2i(0, 0)
                    )
                ))
            }
            (PropertyType::Vector3, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector3 = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector3(
                            value.x,
                            value.y,
                            value.z,
                        )
                    })
                ))
            }
            (PropertyType::Vector3, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector3(0.0, 0.0, 0.0)
                    )
                ))
            }
            (PropertyType::Vector3i, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector3i = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector3i(
                            value.x,
                            value.y,
                            value.z,
                        )
                    })
                ))
            }
            (PropertyType::Vector3i, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector3i(0, 0, 0)
                    )
                ))
            }
            (PropertyType::Vector4, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector4 = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector4(
                            value.x,
                            value.y,
                            value.z,
                            value.w,
                        )
                    })
                ))
            }
            (PropertyType::Vector4, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector4(0.0, 0.0, 0.0, 0.0)
                    )
                ))
            }
            (PropertyType::Vector4i, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Vector4i = #value;
                        ::godot_rs::abi::AbiValueV1::from_vector4i(
                            value.x,
                            value.y,
                            value.z,
                            value.w,
                        )
                    })
                ))
            }
            (PropertyType::Vector4i, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_vector4i(0, 0, 0, 0)
                    )
                ))
            }
            (PropertyType::Rect2, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Rect2 = #value;
                        ::godot_rs::abi::AbiValueV1::from_rect2(
                            value.position.x,
                            value.position.y,
                            value.size.x,
                            value.size.y,
                        )
                    })
                ))
            }
            (PropertyType::Rect2, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_rect2(0.0, 0.0, 0.0, 0.0)
                    )
                ))
            }
            (PropertyType::Rect2i, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Rect2i = #value;
                        ::godot_rs::abi::AbiValueV1::from_rect2i(
                            value.position.x,
                            value.position.y,
                            value.size.x,
                            value.size.y,
                        )
                    })
                ))
            }
            (PropertyType::Rect2i, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_rect2i(0, 0, 0, 0)
                    )
                ))
            }
            (PropertyType::Quaternion, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Quaternion = #value;
                        ::godot_rs::abi::AbiValueV1::from_quaternion(
                            value.x,
                            value.y,
                            value.z,
                            value.w,
                        )
                    })
                ))
            }
            (PropertyType::Quaternion, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_quaternion(0.0, 0.0, 0.0, 1.0)
                    )
                ))
            }
            (PropertyType::Plane, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Plane = #value;
                        ::godot_rs::abi::AbiValueV1::from_plane(
                            value.normal.x,
                            value.normal.y,
                            value.normal.z,
                            value.d,
                        )
                    })
                ))
            }
            (PropertyType::Plane, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_plane(0.0, 0.0, 0.0, 0.0)
                    )
                ))
            }
            (PropertyType::Transform2D, default) => {
                let value = default
                    .cloned()
                    .unwrap_or_else(|| syn::parse_quote!(::godot_rs::math::Transform2D::IDENTITY));
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::FixedMath({
                        let value: ::godot_rs::math::Transform2D = #value;
                        ::godot_rs::abi::AbiFixedMathDefaultV1::new(6, [
                            value.x.x.to_bits(), value.x.y.to_bits(),
                            value.y.x.to_bits(), value.y.y.to_bits(),
                            value.origin.x.to_bits(), value.origin.y.to_bits(),
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        ])
                    })
                ))
            }
            (PropertyType::Aabb, default) => {
                let value = default.cloned().unwrap_or_else(|| {
                    syn::parse_quote!(::godot_rs::math::Aabb::new(
                        ::godot_rs::math::Vector3::ZERO,
                        ::godot_rs::math::Vector3::ZERO,
                    ))
                });
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::FixedMath({
                        let value: ::godot_rs::math::Aabb = #value;
                        ::godot_rs::abi::AbiFixedMathDefaultV1::new(6, [
                            value.position.x.to_bits(), value.position.y.to_bits(),
                            value.position.z.to_bits(), value.size.x.to_bits(),
                            value.size.y.to_bits(), value.size.z.to_bits(),
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                        ])
                    })
                ))
            }
            (PropertyType::Basis, default) => {
                let value = default
                    .cloned()
                    .unwrap_or_else(|| syn::parse_quote!(::godot_rs::math::Basis::IDENTITY));
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::FixedMath({
                        let value: ::godot_rs::math::Basis = #value;
                        ::godot_rs::abi::AbiFixedMathDefaultV1::new(9, [
                            value.x.x.to_bits(), value.x.y.to_bits(), value.x.z.to_bits(),
                            value.y.x.to_bits(), value.y.y.to_bits(), value.y.z.to_bits(),
                            value.z.x.to_bits(), value.z.y.to_bits(), value.z.z.to_bits(),
                            0, 0, 0, 0, 0, 0, 0,
                        ])
                    })
                ))
            }
            (PropertyType::Transform3D, default) => {
                let value = default
                    .cloned()
                    .unwrap_or_else(|| syn::parse_quote!(::godot_rs::math::Transform3D::IDENTITY));
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::FixedMath({
                        let value: ::godot_rs::math::Transform3D = #value;
                        ::godot_rs::abi::AbiFixedMathDefaultV1::new(12, [
                            value.basis.x.x.to_bits(), value.basis.x.y.to_bits(),
                            value.basis.x.z.to_bits(), value.basis.y.x.to_bits(),
                            value.basis.y.y.to_bits(), value.basis.y.z.to_bits(),
                            value.basis.z.x.to_bits(), value.basis.z.y.to_bits(),
                            value.basis.z.z.to_bits(), value.origin.x.to_bits(),
                            value.origin.y.to_bits(), value.origin.z.to_bits(),
                            0, 0, 0, 0,
                        ])
                    })
                ))
            }
            (PropertyType::Projection, default) => {
                let value = default
                    .cloned()
                    .unwrap_or_else(|| syn::parse_quote!(::godot_rs::math::Projection::IDENTITY));
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::FixedMath({
                        let value: ::godot_rs::math::Projection = #value;
                        ::godot_rs::abi::AbiFixedMathDefaultV1::new(16, [
                            value.x.x.to_bits(), value.x.y.to_bits(),
                            value.x.z.to_bits(), value.x.w.to_bits(),
                            value.y.x.to_bits(), value.y.y.to_bits(),
                            value.y.z.to_bits(), value.y.w.to_bits(),
                            value.z.x.to_bits(), value.z.y.to_bits(),
                            value.z.z.to_bits(), value.z.w.to_bits(),
                            value.w.x.to_bits(), value.w.y.to_bits(),
                            value.w.z.to_bits(), value.w.w.to_bits(),
                        ])
                    })
                ))
            }
            (PropertyType::Color, Some(value)) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar({
                        let value: ::godot_rs::math::Color = #value;
                        ::godot_rs::abi::AbiValueV1::from_color(
                            value.r,
                            value.g,
                            value.b,
                            value.a,
                        )
                    })
                ))
            }
            (PropertyType::Color, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_color(0.0, 0.0, 0.0, 1.0)
                    )
                ))
            }
            (PropertyType::NodeRef | PropertyType::ResourceRef, None) => {
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::Scalar(
                        ::godot_rs::abi::AbiValueV1::from_object_id(0)
                    )
                ))
            }
            (PropertyType::NodeRef | PropertyType::ResourceRef, Some(_)) => {
                unreachable!("object-reference defaults were rejected")
            }
            (PropertyType::GodotInteger, default) => {
                let default = default.map_or_else(
                    || {
                        quote!(
                            <#field_type as ::godot_rs::engine::GodotIntegerValue>::
                                __from_raw(
                                    <#field_type as ::godot_rs::engine::GodotIntegerValue>::
                                        PROPERTY_DEFAULT_RAW
                                )
                        )
                    },
                    |value| quote!(#value),
                );
                quote!(::core::option::Option::Some(
                    ::godot_rs::script::PropertyDefault::GodotInteger({
                        unsafe extern "C" fn default_value() -> u64 {
                            let value: #field_type = #default;
                            <#field_type as ::godot_rs::engine::GodotIntegerValue>::__raw(value)
                        }
                        default_value
                    })
                ))
            }
            (
                PropertyType::Array
                | PropertyType::Dictionary
                | PropertyType::PackedByteArray
                | PropertyType::PackedInt32Array
                | PropertyType::PackedInt64Array
                | PropertyType::PackedFloat32Array
                | PropertyType::PackedFloat64Array
                | PropertyType::PackedStringArray
                | PropertyType::PackedVector2Array
                | PropertyType::PackedVector3Array
                | PropertyType::PackedColorArray
                | PropertyType::PackedVector4Array,
                None,
            ) => quote!(::core::option::Option::Some(
                ::godot_rs::script::PropertyDefault::Empty(
                    ::godot_rs::script::property_value_type(#abi_type)
                )
            )),
            (
                PropertyType::Array
                | PropertyType::Dictionary
                | PropertyType::PackedByteArray
                | PropertyType::PackedInt32Array
                | PropertyType::PackedInt64Array
                | PropertyType::PackedFloat32Array
                | PropertyType::PackedFloat64Array
                | PropertyType::PackedStringArray
                | PropertyType::PackedVector2Array
                | PropertyType::PackedVector3Array
                | PropertyType::PackedColorArray
                | PropertyType::PackedVector4Array,
                Some(_),
            ) => unreachable!("container defaults were rejected"),
        };
        let hint = if type_ == PropertyType::GodotInteger {
            quote! {
                if <#field_type as ::godot_rs::engine::GodotIntegerValue>::SIGNED {
                    ::godot_rs::abi::ABI_PROPERTY_HINT_ENUM
                } else {
                    ::godot_rs::abi::ABI_PROPERTY_HINT_FLAGS
                }
            }
        } else {
            let hint = metadata.property_hint;
            quote!(#hint)
        };
        let hint_string = metadata
            .property_hint_expression
            .clone()
            .unwrap_or_else(|| {
                metadata.property_object_class.as_ref().map_or_else(
                    || {
                        let hint = LitStr::new(&metadata.property_hint_string, Span::call_site());
                        quote!(#hint)
                    },
                    |class| quote!(<#class as ::godot_rs::engine::GodotClass>::CLASS_NAME),
                )
            });
        let group = metadata.property_group.as_ref().map(|group| {
            let group = LitStr::new(group, Span::call_site());
            quote!(::core::option::Option::Some(#group))
        });
        let group = group.unwrap_or_else(|| quote!(::core::option::Option::None));
        let typed_array_element = metadata.typed_array_class.as_ref().map(|class| quote!(
            ::core::option::Option::Some(<#class as ::godot_rs::engine::GodotClass>::CLASS_NAME)
        )).or_else(|| metadata.typed_array_element.as_ref().map(|element| {
            let element = LitStr::new(element, Span::call_site());
            quote!(::core::option::Option::Some(#element))
        }));
        let typed_array_element =
            typed_array_element.unwrap_or_else(|| quote!(::core::option::Option::None));
        let integer_options = if type_ == PropertyType::GodotInteger {
            quote!(::core::option::Option::Some(
                <#field_type as ::godot_rs::engine::GodotIntegerValue>::PROPERTY_OPTIONS
            ))
        } else {
            quote!(::core::option::Option::None)
        };
        let encoded = encode_property_schema(
            metadata.property_group.as_deref().unwrap_or_default(),
            &hint_string,
            &typed_array_element,
        );
        let base_usage = if metadata.kind == FieldKind::Property {
            quote!(::godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_STORAGE)
        } else if metadata.property_node_usage {
            quote!(
                ::godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_DEFAULT
                    | ::godot_rs::abi::ABI_PROPERTY_USAGE_NODE_PATH_FROM_SCENE_ROOT
            )
        } else {
            quote!(::godot_rs::abi::ABI_PROPERTY_USAGE_SCRIPT_DEFAULT)
        };
        let usage = if metadata.property_read_only {
            quote!(#base_usage | ::godot_rs::abi::ABI_PROPERTY_USAGE_READ_ONLY)
        } else {
            base_usage
        };
        quote! {
            ::core::option::Option::Some(::godot_rs::script::PropertyDescriptor {
                type_: #abi_type,
                hint: #hint,
                hint_string: #hint_string,
                typed_array_element: #typed_array_element,
                integer_options: #integer_options,
                usage: #usage,
                group: #group,
                default_value: #default_value,
                encoded: #encoded,
            })
        }
    });
    let property = property.unwrap_or_else(|| quote!(::core::option::Option::None));
    let node = if metadata.kind == FieldKind::Node {
        let path = LitStr::new(
            metadata
                .node_path
                .as_deref()
                .expect("node path was validated"),
            Span::call_site(),
        );
        let class = metadata
            .node_class
            .as_ref()
            .expect("node class was validated");
        let optional = metadata.node_optional;
        quote! {
            ::core::option::Option::Some(::godot_rs::script::NodeDescriptor {
                path: #path,
                class_name: <#class as ::godot_rs::engine::GodotClass>::CLASS_NAME,
                optional: #optional,
            })
        }
    } else {
        quote!(::core::option::Option::None)
    };
    let signal = if metadata.kind == FieldKind::Signal {
        let arguments = metadata.signal_arguments.iter().map(|argument| {
            let name = &argument.name;
            let type_ = abi_value_type(&argument.type_)
                .expect("signal argument type was validated before descriptor generation");
            quote! {
                ::godot_rs::script::SignalArgumentDescriptor {
                    name: stringify!(#name),
                    type_: #type_,
                }
            }
        });
        let abi_arguments = metadata.signal_arguments.iter().map(|argument| {
            let name = &argument.name;
            let type_ = abi_value_type(&argument.type_)
                .expect("signal argument type was validated before descriptor generation");
            quote! {
                ::godot_rs::abi::AbiSignalArgumentDescriptorV1 {
                    name: ::godot_rs::abi::AbiByteSlice::from_static(stringify!(#name)),
                    type_: #type_,
                    reserved_flags: 0,
                }
            }
        });
        quote! {
            ::core::option::Option::Some({
                const ARGUMENTS: &'static [
                    ::godot_rs::script::SignalArgumentDescriptor
                ] = &[#(#arguments),*];
                const ABI_ARGUMENTS: &'static [
                    ::godot_rs::abi::AbiSignalArgumentDescriptorV1
                ] = &[#(#abi_arguments),*];
                ::godot_rs::script::SignalDescriptor {
                    arguments: ARGUMENTS,
                    abi_arguments: ABI_ARGUMENTS,
                }
            })
        }
    } else {
        quote!(::core::option::Option::None)
    };
    Ok(quote! {
        ::godot_rs::script::FieldDescriptor {
            name: stringify!(#name),
            rust_type: #rust_type,
            kind: #kind,
            options: #options,
            default: #default,
            reload: #reload,
            reload_value_type: #reload_value_type,
            property: #property,
            node: #node,
            signal: #signal,
        }
    })
}

fn encode_property_schema(
    group: &str,
    hint_string: &TokenStream2,
    typed_array_element: &TokenStream2,
) -> TokenStream2 {
    quote!({
        const PARTS: &[&str] = &[#group, #hint_string, match #typed_array_element {
            ::core::option::Option::Some(value) => value,
            ::core::option::Option::None => "",
        }];
        const PREFIX: &[u8] = b"gdrs-property-v2:";
        const fn digits(mut value: usize) -> usize {
            let mut count = 1;
            while value >= 10 {
                count += 1;
                value /= 10;
            }
            count
        }
        const LENGTH: usize = {
            let mut length = PREFIX.len();
            let mut part = 0;
            while part < PARTS.len() {
                length += digits(PARTS[part].len()) + 1 + PARTS[part].len();
                part += 1;
            }
            length
        };
        const BYTES: [u8; LENGTH] = {
            let mut bytes = [0; LENGTH];
            let mut offset = 0;
            while offset < PREFIX.len() {
                bytes[offset] = PREFIX[offset];
                offset += 1;
            }
            let mut part = 0;
            while part < PARTS.len() {
                let mut value = PARTS[part].len();
                let count = digits(value);
                let mut digit = 0;
                while digit < count {
                    bytes[offset + count - digit - 1] = b'0' + (value % 10) as u8;
                    value /= 10;
                    digit += 1;
                }
                offset += count;
                bytes[offset] = b':';
                offset += 1;
                part += 1;
            }
            part = 0;
            while part < PARTS.len() {
                let text = PARTS[part].as_bytes();
                let mut index = 0;
                while index < text.len() {
                    bytes[offset] = text[index];
                    offset += 1;
                    index += 1;
                }
                part += 1;
            }
            bytes
        };
        match ::core::str::from_utf8(&BYTES) {
            ::core::result::Result::Ok(value) => value,
            ::core::result::Result::Err(_) => panic!("Godot property schemas are UTF-8"),
        }
    })
}

fn expand_impl(arguments: TokenStream2, mut item: ItemImpl) -> syn::Result<TokenStream2> {
    if !arguments.is_empty() {
        return Err(Error::new_spanned(
            arguments,
            "the impl form is written as `#[script]` without arguments",
        ));
    }
    let virtual_trait = item
        .trait_
        .as_ref()
        .map(|(_, path, _)| path)
        .filter(|path| {
            path.segments
                .last()
                .is_some_and(|segment| segment.ident.to_string().ends_with("Virtual"))
        })
        .cloned();
    if item.trait_.is_some() && virtual_trait.is_none() {
        return Err(Error::new_spanned(
            &item,
            "`#[script]` trait impls must use a generated `*Virtual` Godot override trait",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &item.generics,
            "script impl blocks cannot have generic parameters",
        ));
    }

    let self_type = item.self_ty.clone();
    let type_name = self_type_name(&self_type)?;
    let mut descriptors = Vec::new();
    let mut wrappers = Vec::new();
    let mut invocations = Vec::new();
    let mut proxies = Vec::new();

    for impl_item in &mut item.items {
        match impl_item {
            ImplItem::Fn(method) => {
                if let Some(expansion) =
                    method_descriptor(&self_type, &type_name, method, virtual_trait.as_ref())?
                {
                    descriptors.push(expansion.descriptor);
                    if let Some(wrapper) = expansion.wrapper {
                        wrappers.push(wrapper);
                    }
                    if let Some(invocation) = expansion.invocation {
                        invocations.push(invocation);
                    }
                    if let Some(proxy) = expansion.proxy {
                        proxies.push(proxy);
                    }
                }
            }
            ImplItem::Const(constant)
                if virtual_trait.is_none() && matches!(constant.vis, Visibility::Public(_)) =>
            {
                let expansion = constant_descriptor(&self_type, constant)?;
                descriptors.push(expansion.descriptor);
                invocations.push(
                    expansion
                        .invocation
                        .expect("constant descriptors always have an invocation"),
                );
            }
            _ => {}
        }
    }

    Ok(quote! {
        #item

        #(#wrappers)*
        #(#proxies)*

        ::godot_rs::inventory::submit! {
            ::godot_rs::script::MethodBlock {
                script_type_id: ::godot_rs::script::script_type_id::<#self_type>,
                methods: &[#(#descriptors),*],
                invoke: |
                    state: *mut ::core::ffi::c_void,
                    method_id: u64,
                    arguments: *const ::godot_rs::abi::AbiValueV1,
                    argument_count: u32,
                    output: *mut ::godot_rs::abi::AbiValueV1,
                | match method_id {
                        #(#invocations)*
                        _ => ::godot_rs::abi::AbiCallResult::failure(
                            ::godot_rs::abi::AbiStatus::Unsupported,
                            "reflected method ID is not present in this script block",
                        ),
                    },
            }
        }
    })
}

fn constant_descriptor(self_type: &Type, constant: &ImplItemConst) -> syn::Result<MethodExpansion> {
    if !constant.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &constant.generics,
            "Godot-visible constants cannot have generic parameters",
        ));
    }
    let name = &constant.ident;
    let rust_name = name.to_string();
    let godot_name = rust_name.strip_prefix("r#").unwrap_or(&rust_name);
    if !is_godot_class_identifier(godot_name) {
        return Err(Error::new_spanned(
            name,
            "a Godot-visible constant must use a valid Godot identifier",
        ));
    }
    let exported_name = LitStr::new(godot_name, name.span());
    let id = fnv1a(godot_name.as_bytes());
    let signature = token_string(constant);
    let (return_type, result) = if is_str_reference(&constant.ty) {
        (
            quote!(::godot_rs::abi::AbiValueType::STRING),
            quote!((<#self_type>::#name).to_owned()),
        )
    } else {
        (abi_value_type(&constant.ty)?, quote!(<#self_type>::#name))
    };
    let descriptor = quote! {
        ::godot_rs::script::MethodDescriptor {
            id: #id,
            name: #exported_name,
            rust_signature: #signature,
            kind: ::godot_rs::script::MethodKind::Constant,
            receiver: ::godot_rs::script::ReceiverKind::Static,
            argument_count: 0,
            argument_types: &[],
            arguments: &[],
            abi_arguments: &[],
            abi_argument_classes: &[],
            default_arguments: &[],
            vararg: false,
            abi_extensions: ::godot_rs::script::method_extensions(
                &[],
                ::core::option::Option::None,
                &[],
                false,
            ),
            return_type: #return_type,
            return_class: ::core::option::Option::None,
            options: "",
            rpc: ::core::option::Option::None,
            callback: ::godot_rs::script::MethodCallback::None,
        }
    };
    let invocation = quote! {
        #id => {
            if argument_count != 0 {
                return ::godot_rs::abi::AbiCallResult::failure(
                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                    "script constants do not accept arguments",
                );
            }
            ::godot_rs::script::encode_method_result(#result, output)
        }
    };
    Ok(MethodExpansion {
        descriptor,
        wrapper: None,
        invocation: Some(invocation),
        proxy: None,
    })
}

fn is_str_reference(type_: &Type) -> bool {
    let Type::Reference(reference) = type_ else {
        return false;
    };
    matches!(
        reference.elem.as_ref(),
        Type::Path(path) if path.path.is_ident("str")
    )
}

struct MethodExpansion {
    descriptor: TokenStream2,
    wrapper: Option<TokenStream2>,
    invocation: Option<TokenStream2>,
    proxy: Option<TokenStream2>,
}

struct ExplicitSelfType<'a>(&'a Type);

impl VisitMut for ExplicitSelfType<'_> {
    fn visit_type_mut(&mut self, type_: &mut Type) {
        if matches!(type_, Type::Path(path) if path.qself.is_none() && path.path.is_ident("Self")) {
            *type_ = self.0.clone();
        } else {
            syn::visit_mut::visit_type_mut(self, type_);
        }
    }
}

fn method_descriptor(
    self_type: &Type,
    type_name: &syn::Ident,
    method: &mut ImplItemFn,
    virtual_trait: Option<&Path>,
) -> syn::Result<Option<MethodExpansion>> {
    let func = take_method_attribute(&mut method.attrs, "func")?;
    let rpc = take_method_attribute(&mut method.attrs, "rpc")?;
    if func.is_some() && rpc.is_some() {
        return Err(Error::new_spanned(
            &method.sig.ident,
            "an RPC method is already Godot-visible; remove `#[func]`",
        ));
    }
    if virtual_trait.is_some() && (func.is_some() || rpc.is_some()) {
        return Err(Error::new_spanned(
            &method.sig.ident,
            "generated Godot virtual methods are registered automatically",
        ));
    }
    let func_name = func
        .as_ref()
        .map(parse_func_arguments)
        .transpose()?
        .flatten();
    let rpc_config = rpc.as_ref().map(parse_rpc_arguments).transpose()?;

    let lifecycle = virtual_trait
        .is_none()
        .then(|| Lifecycle::from_name(&method.sig.ident.to_string()))
        .flatten();
    if lifecycle.is_some() && (func.is_some() || rpc.is_some()) {
        return Err(Error::new_spanned(
            &method.sig.ident,
            "Godot lifecycle callbacks are registered automatically",
        ));
    }
    if lifecycle.is_none() && func.is_none() && rpc.is_none() && virtual_trait.is_none() {
        let message = match method.sig.ident.to_string().as_str() {
            "_init" => Some(
                "`_init` is not a Rust script callback; initialize state with `#[init(default = expression)]` and access scene nodes in `_ready`",
            ),
            "_notification"
            | "_unhandled_key_input"
            | "_shortcut_input"
            | "_get_configuration_warnings"
            | "_get"
            | "_set"
            | "_get_property_list"
            | "_validate_property"
            | "_property_can_revert"
            | "_property_get_revert"
            | "_to_string" => Some(
                "this Godot callback is not automatically registered in an inherent impl; implement its generated `*Virtual` trait with `#[script]`",
            ),
            _ => None,
        };
        if let Some(message) = message {
            return Err(Error::new_spanned(&method.sig.ident, message));
        }
        return Ok(None);
    }
    if !method.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &method.sig.generics,
            "Godot-visible methods cannot have generic parameters",
        ));
    }
    if method.sig.asyncness.is_some()
        || method.sig.unsafety.is_some()
        || method.sig.constness.is_some()
        || method.sig.abi.is_some()
        || method.sig.variadic.is_some()
    {
        return Err(Error::new_spanned(
            &method.sig,
            "Godot-visible methods must be synchronous safe Rust functions",
        ));
    }

    let mut parameter_defaults = method
        .sig
        .inputs
        .iter_mut()
        .filter_map(|argument| match argument {
            FnArg::Typed(argument) => Some(take_parameter_default(argument)),
            FnArg::Receiver(_) => None,
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let is_vararg = method
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Typed(argument) => Some(argument),
            FnArg::Receiver(_) => None,
        })
        .next_back()
        .is_some_and(|argument| is_variant_slice(&argument.ty));
    if is_vararg && parameter_defaults.last().is_some_and(Option::is_some) {
        return Err(Error::new_spanned(
            &method.sig.inputs,
            "the trailing `&[Variant]` variable-argument slice cannot have a default",
        ));
    }
    if is_vararg {
        parameter_defaults.pop();
    }
    let first_default = parameter_defaults.iter().position(Option::is_some);
    if first_default.is_some_and(|index| parameter_defaults[index..].iter().any(Option::is_none)) {
        return Err(Error::new_spanned(
            &method.sig.inputs,
            "default arguments must be trailing; add defaults to every following fixed argument",
        ));
    }
    if (!parameter_defaults.iter().all(Option::is_none) || is_vararg) && lifecycle.is_some() {
        return Err(Error::new_spanned(
            &method.sig.inputs,
            "Godot lifecycle callbacks cannot declare defaults or variable arguments",
        ));
    }
    if parameter_defaults.iter().any(Option::is_some) && virtual_trait.is_some() {
        return Err(Error::new_spanned(
            &method.sig.inputs,
            "generated Godot virtual overrides cannot redeclare engine defaults",
        ));
    }

    let signature = token_string(&method.sig);
    // Generated metadata and proxies live outside the user's impl, where Self
    // either has no meaning or names a proxy. Keep the original method intact.
    let mut generated_method = method.clone();
    let mut explicit_self = ExplicitSelfType(self_type);
    for argument in &mut generated_method.sig.inputs {
        if let FnArg::Typed(argument) = argument {
            explicit_self.visit_type_mut(&mut argument.ty);
        }
    }
    if let ReturnType::Type(_, type_) = &mut generated_method.sig.output {
        explicit_self.visit_type_mut(type_);
    }
    let method = &generated_method;

    let receiver = receiver_kind(method)?;
    let mut typed_arguments: Vec<_> = method
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Typed(argument) => Some(argument),
            FnArg::Receiver(_) => None,
        })
        .collect();
    if is_vararg {
        typed_arguments.pop();
    }
    let argument_names = typed_arguments
        .iter()
        .map(|argument| {
            let Pat::Ident(pattern) = argument.pat.as_ref() else {
                return Err(Error::new_spanned(
                    &argument.pat,
                    "Godot-visible method arguments must use simple names",
                ));
            };
            Ok(pattern.ident.clone())
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let argument_count = typed_arguments.len();
    let argument_count = u16::try_from(argument_count).map_err(|_| {
        Error::new_spanned(
            &method.sig.inputs,
            "Godot-visible method has too many arguments",
        )
    })?;

    let method_name = &method.sig.ident;
    let method_name_ident = method_name.to_string();
    let rust_method_name = method_name_ident
        .strip_prefix("r#")
        .unwrap_or(&method_name_ident);
    let method_name_string = func_name
        .as_deref()
        .or_else(|| {
            rpc_config
                .as_ref()
                .and_then(|config| config.name.as_deref())
        })
        .unwrap_or(rust_method_name);
    let method_id = fnv1a(method_name_string.as_bytes());
    let exported_method_name = LitStr::new(method_name_string, method_name.span());
    let options = rpc.as_ref().map(attribute_options).unwrap_or_default();
    let options = LitStr::new(&options, Span::call_site());
    let receiver_tokens = match receiver {
        Receiver::Shared => quote!(::godot_rs::script::ReceiverKind::Shared),
        Receiver::Mutable => quote!(::godot_rs::script::ReceiverKind::Mutable),
        Receiver::Static => quote!(::godot_rs::script::ReceiverKind::Static),
    };
    let rpc_tokens = rpc_config.as_ref().map_or_else(
        || quote!(::core::option::Option::None),
        ParsedRpcConfig::tokens,
    );

    let (kind, callback, wrapper, argument_types, return_type, invocation) =
        if let Some(lifecycle) = lifecycle {
            lifecycle.validate(method)?;
            let wrapper_name = format_ident!(
                "__godot_rs_{}_{}",
                type_name.to_string().to_lowercase(),
                method.sig.ident,
                span = Span::call_site()
            );
            let (callback, wrapper) = lifecycle.wrapper(self_type, method_name, &wrapper_name);
            let argument_types = lifecycle.argument_types();
            (
                lifecycle.kind_tokens(),
                callback,
                Some(wrapper),
                argument_types,
                quote!(::godot_rs::abi::AbiValueType::NIL),
                None,
            )
        } else if rpc.is_some() {
            let (argument_types, return_type, invocation) = reflected_method_invocation(
                self_type,
                method,
                receiver,
                method_id,
                virtual_trait,
                is_vararg,
            )?;
            (
                quote!(::godot_rs::script::MethodKind::Rpc),
                quote!(::godot_rs::script::MethodCallback::None),
                None,
                argument_types,
                return_type,
                Some(invocation),
            )
        } else {
            let (argument_types, return_type, invocation) = reflected_method_invocation(
                self_type,
                method,
                receiver,
                method_id,
                virtual_trait,
                is_vararg,
            )?;
            (
                quote!(::godot_rs::script::MethodKind::Func),
                quote!(::godot_rs::script::MethodCallback::None),
                None,
                argument_types,
                return_type,
                Some(invocation),
            )
        };
    let argument_classes = if matches!(
        lifecycle,
        Some(Lifecycle::Input | Lifecycle::UnhandledInput)
    ) {
        vec![Some(quote!("InputEvent"))]
    } else {
        typed_arguments
            .iter()
            .map(|argument| reflected_value_metadata(&argument.ty))
            .collect::<Vec<_>>()
    };
    let arguments = argument_names
        .iter()
        .zip(&argument_types)
        .zip(&argument_classes)
        .map(|((name, type_), class_name)| {
            let class_name = class_name.as_ref().map_or_else(
                || quote!(::core::option::Option::None),
                |class_name| quote!(::core::option::Option::Some(#class_name)),
            );
            quote! {
                ::godot_rs::script::MethodArgumentDescriptor {
                    name: stringify!(#name),
                    type_: #type_,
                    class_name: #class_name,
                }
            }
        })
        .collect::<Vec<_>>();
    let abi_arguments = argument_names
        .iter()
        .zip(&argument_types)
        .map(|(name, type_)| {
            quote! {
                ::godot_rs::abi::AbiMethodArgumentDescriptorV1 {
                    name: ::godot_rs::abi::AbiByteSlice::from_static(stringify!(#name)),
                    type_: #type_,
                    reserved_flags: 0,
                }
            }
        })
        .collect::<Vec<_>>();
    let abi_argument_classes = argument_classes
        .iter()
        .map(|class_name| {
            class_name.as_ref().map_or_else(
                || quote!(::godot_rs::abi::AbiByteSlice::EMPTY),
                |class_name| quote!(::godot_rs::abi::AbiByteSlice::from_static(#class_name)),
            )
        })
        .collect::<Vec<_>>();
    let default_arguments = typed_arguments
        .iter()
        .zip(parameter_defaults)
        .filter_map(|(argument, default)| {
            let default = default?;
            let argument_type = &argument.ty;
            Some(quote! {
                ::core::option::Option::Some({
                    unsafe extern "C" fn default_argument(
                        output: *mut ::godot_rs::abi::AbiValueV1,
                    ) -> ::godot_rs::abi::AbiCallResult {
                        ::godot_rs::script::catch_abi_panic(|| {
                            let value: #argument_type = #default;
                            ::godot_rs::script::encode_method_result(value, output)
                        })
                    }
                    default_argument
                })
            })
        })
        .collect::<Vec<_>>();
    let return_class = reflected_return_value_metadata(&method.sig.output).map_or_else(
        || quote!(::core::option::Option::None),
        |class_name| quote!(::core::option::Option::Some(#class_name)),
    );

    let descriptor = quote! {{
        const ABI_ARGUMENT_CLASSES: &[::godot_rs::abi::AbiByteSlice] =
            &[#(#abi_argument_classes),*];
        const DEFAULT_ARGUMENTS: &[::godot_rs::abi::AbiMethodDefaultFn] =
            &[#(#default_arguments),*];
        ::godot_rs::script::MethodDescriptor {
            id: #method_id,
            name: #exported_method_name,
            rust_signature: #signature,
            kind: #kind,
            receiver: #receiver_tokens,
            argument_count: #argument_count,
            argument_types: &[#(#argument_types),*],
            arguments: &[#(#arguments),*],
            abi_arguments: &[#(#abi_arguments),*],
            abi_argument_classes: ABI_ARGUMENT_CLASSES,
            default_arguments: DEFAULT_ARGUMENTS,
            vararg: #is_vararg,
            abi_extensions: ::godot_rs::script::method_extensions(
                ABI_ARGUMENT_CLASSES,
                #return_class,
                DEFAULT_ARGUMENTS,
                #is_vararg,
            ),
            return_type: #return_type,
            return_class: #return_class,
            options: #options,
            rpc: #rpc_tokens,
            callback: #callback,
        }
    }};
    Ok(Some(MethodExpansion {
        descriptor,
        wrapper,
        invocation,
        proxy: if lifecycle.is_none() && virtual_trait.is_none() {
            Some(typed_script::method_api(
                self_type,
                type_name,
                method,
                method_id,
                &exported_method_name,
                is_vararg,
            )?)
        } else {
            None
        },
    }))
}

fn reflected_method_invocation(
    self_type: &Type,
    method: &ImplItemFn,
    receiver: Receiver,
    method_id: u64,
    virtual_trait: Option<&Path>,
    is_vararg: bool,
) -> syn::Result<(Vec<TokenStream2>, TokenStream2, TokenStream2)> {
    let mut argument_types = Vec::new();
    let mut decoders = Vec::new();
    let mut names = Vec::new();
    let mut typed_arguments = method
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Typed(argument) => Some(argument),
            FnArg::Receiver(_) => None,
        })
        .collect::<Vec<_>>();
    let vararg_name = if is_vararg {
        let argument = typed_arguments
            .pop()
            .expect("a detected variable-argument slice is present");
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(Error::new_spanned(
                &argument.pat,
                "Godot-visible method arguments must use simple names",
            ));
        };
        Some(pattern.ident.clone())
    } else {
        None
    };
    for (index, argument) in typed_arguments.into_iter().enumerate() {
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(Error::new_spanned(
                &argument.pat,
                "Godot-visible method arguments must use simple names",
            ));
        };
        let name = &pattern.ident;
        let argument_type = &argument.ty;
        argument_types.push(abi_value_type(argument_type)?);
        let index = u32::try_from(index).expect("method argument count was bounded");
        decoders.push(quote! {
            let #name: #argument_type = match unsafe {
                ::godot_rs::script::decode_method_argument(
                    arguments,
                    argument_count,
                    #index,
                )
            } {
                Ok(value) => value,
                Err(error) => return error,
            };
        });
        names.push(name);
    }
    let return_type = return_abi_value_type(&method.sig.output)?;
    let method_name = &method.sig.ident;
    let call_arguments = if let Some(vararg_name) = &vararg_name {
        quote!(#(#names,)* #vararg_name.as_slice())
    } else {
        quote!(#(#names),*)
    };
    let call = match (virtual_trait, receiver) {
        (Some(trait_), Receiver::Shared | Receiver::Mutable) => {
            quote!(<#self_type as #trait_>::#method_name(script_state, #call_arguments))
        }
        (Some(_), Receiver::Static) => {
            return Err(Error::new_spanned(
                &method.sig,
                "Godot virtual methods must start with `&mut self`",
            ));
        }
        (None, Receiver::Shared | Receiver::Mutable) => {
            quote!(script_state.#method_name(#call_arguments))
        }
        (None, Receiver::Static) => quote!(<#self_type>::#method_name(#call_arguments)),
    };
    let state_binding = match receiver {
        Receiver::Shared => quote! {
            if state.is_null() {
                return ::godot_rs::abi::AbiCallResult::failure(
                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                    "instance method received a null script state",
                );
            }
            // SAFETY: The Host pairs this method block with live state for the
            // same script type and holds a shared borrow for this invocation.
            let script_state = unsafe { &*state.cast::<#self_type>() };
        },
        Receiver::Mutable => quote! {
            if state.is_null() {
                return ::godot_rs::abi::AbiCallResult::failure(
                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                    "instance method received a null script state",
                );
            }
            // SAFETY: The Host pairs this method block with live state for the
            // same script type and holds an exclusive borrow for this invocation.
            let script_state = unsafe { &mut *state.cast::<#self_type>() };
        },
        Receiver::Static => quote! {},
    };
    let argument_count = u32::try_from(argument_types.len()).expect("method count was bounded");
    let count_check = if is_vararg {
        quote!(argument_count < #argument_count)
    } else {
        quote!(argument_count != #argument_count)
    };
    let vararg_decoder = vararg_name.map(|name| {
        quote! {
            let mut #name = ::std::vec::Vec::<::godot_rs::variant::Variant>::with_capacity(
                argument_count.saturating_sub(#argument_count) as usize,
            );
            for index in #argument_count..argument_count {
                let value = match unsafe {
                    ::godot_rs::script::decode_method_argument(
                        arguments,
                        argument_count,
                        index,
                    )
                } {
                    Ok(value) => value,
                    Err(error) => return error,
                };
                #name.push(value);
            }
        }
    });
    let invocation = quote! {
        #method_id => {
            if #count_check {
                return ::godot_rs::abi::AbiCallResult::failure(
                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                    "reflected method argument count does not match its descriptor",
                );
            }
            #state_binding
            #(#decoders)*
            #vararg_decoder
            let result = #call;
            ::godot_rs::script::encode_method_result(result, output)
        }
    };
    Ok((argument_types, return_type, invocation))
}

fn abi_value_type(value: &Type) -> syn::Result<TokenStream2> {
    let name = outer_type_name(value);
    let tokens = match name.as_deref() {
        Some("bool") => quote!(::godot_rs::abi::AbiValueType::BOOL),
        Some("i8" | "i16" | "i32" | "i64") => quote!(::godot_rs::abi::AbiValueType::I64),
        Some("u8" | "u16" | "u32" | "u64" | "char") => {
            quote!(::godot_rs::abi::AbiValueType::U64)
        }
        Some("isize" | "usize" | "i128" | "u128") => {
            return Err(Error::new_spanned(
                value,
                "Godot integers have a fixed 64-bit boundary; use `i64` or `u64`",
            ));
        }
        Some("f32" | "f64") => quote!(::godot_rs::abi::AbiValueType::F64),
        Some("String") => quote!(::godot_rs::abi::AbiValueType::STRING),
        Some("StringName") => quote!(::godot_rs::abi::AbiValueType::STRING_NAME),
        Some("NodePath") => quote!(::godot_rs::abi::AbiValueType::NODE_PATH),
        Some("Variant") => quote!(::godot_rs::abi::AbiValueType::VARIANT),
        Some("Array") => quote!(::godot_rs::abi::AbiValueType::ARRAY),
        Some("Dictionary") => quote!(::godot_rs::abi::AbiValueType::DICTIONARY),
        Some("Callable") => quote!(::godot_rs::abi::AbiValueType::CALLABLE),
        Some("Signal") => quote!(::godot_rs::abi::AbiValueType::SIGNAL),
        Some("Vector2") => quote!(::godot_rs::abi::AbiValueType::VECTOR2),
        Some("Vector2i") => quote!(::godot_rs::abi::AbiValueType::VECTOR2I),
        Some("Vector3") => quote!(::godot_rs::abi::AbiValueType::VECTOR3),
        Some("Vector3i") => quote!(::godot_rs::abi::AbiValueType::VECTOR3I),
        Some("Vector4") => quote!(::godot_rs::abi::AbiValueType::VECTOR4),
        Some("Vector4i") => quote!(::godot_rs::abi::AbiValueType::VECTOR4I),
        Some("Rect2") => quote!(::godot_rs::abi::AbiValueType::RECT2),
        Some("Rect2i") => quote!(::godot_rs::abi::AbiValueType::RECT2I),
        Some("Quaternion") => quote!(::godot_rs::abi::AbiValueType::QUATERNION),
        Some("Plane") => quote!(::godot_rs::abi::AbiValueType::PLANE),
        Some("Transform2D") => quote!(::godot_rs::abi::AbiValueType::TRANSFORM2D),
        Some("Aabb") => quote!(::godot_rs::abi::AbiValueType::AABB),
        Some("Basis") => quote!(::godot_rs::abi::AbiValueType::BASIS),
        Some("Transform3D") => quote!(::godot_rs::abi::AbiValueType::TRANSFORM3D),
        Some("Projection") => quote!(::godot_rs::abi::AbiValueType::PROJECTION),
        Some("PackedByteArray") => quote!(::godot_rs::abi::AbiValueType::PACKED_BYTE_ARRAY),
        Some("PackedInt32Array") => quote!(::godot_rs::abi::AbiValueType::PACKED_INT32_ARRAY),
        Some("PackedInt64Array") => quote!(::godot_rs::abi::AbiValueType::PACKED_INT64_ARRAY),
        Some("PackedFloat32Array") => {
            quote!(::godot_rs::abi::AbiValueType::PACKED_FLOAT32_ARRAY)
        }
        Some("PackedFloat64Array") => {
            quote!(::godot_rs::abi::AbiValueType::PACKED_FLOAT64_ARRAY)
        }
        Some("PackedStringArray") => quote!(::godot_rs::abi::AbiValueType::PACKED_STRING_ARRAY),
        Some("PackedVector2Array") => {
            quote!(::godot_rs::abi::AbiValueType::PACKED_VECTOR2_ARRAY)
        }
        Some("PackedVector3Array") => {
            quote!(::godot_rs::abi::AbiValueType::PACKED_VECTOR3_ARRAY)
        }
        Some("PackedColorArray") => quote!(::godot_rs::abi::AbiValueType::PACKED_COLOR_ARRAY),
        Some("PackedVector4Array") => {
            quote!(::godot_rs::abi::AbiValueType::PACKED_VECTOR4_ARRAY)
        }
        Some("Color") => quote!(::godot_rs::abi::AbiValueType::COLOR),
        Some("Rid") => quote!(::godot_rs::abi::AbiValueType::RID),
        _ if reflected_object_class(value).is_some() => {
            quote!(::godot_rs::abi::AbiValueType::OBJECT_ID)
        }
        _ if has_type_arguments(value) => {
            return Err(Error::new_spanned(
                value,
                "unsupported reflected value type; use a supported scalar, text, math, packed array, dynamic container, `Callable`, `Signal`, `Rid`, `ObjectRef<Class>`, or `Option<ObjectRef<Class>>` type",
            ));
        }
        _ => quote!(::godot_rs::script::reflected_integer_value_type::<#value>()),
    };
    Ok(tokens)
}

fn has_type_arguments(value: &Type) -> bool {
    let Type::Path(path) = value else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| !matches!(segment.arguments, PathArguments::None))
}

fn reflected_object_class(value: &Type) -> Option<Type> {
    let Type::Path(path) = value else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident == "Option" {
        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
            return None;
        };
        let GenericArgument::Type(inner) = arguments.args.first()? else {
            return None;
        };
        return (arguments.args.len() == 1)
            .then(|| reflected_object_class(inner))
            .flatten();
    }
    if segment.ident != "ObjectRef" && segment.ident != "NodeRef" && segment.ident != "GodotRef" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(Type::Path(class)) = arguments.args.first()? else {
        return None;
    };
    (arguments.args.len() == 1).then(|| Type::Path(class.clone()))
}

fn reflected_value_metadata(value: &Type) -> Option<TokenStream2> {
    if let Some(class) = reflected_object_class(value) {
        return Some(quote!(<#class as ::godot_rs::engine::GodotClass>::CLASS_NAME));
    }
    let Type::Path(path) = value else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Array" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(element) = arguments.args.first()? else {
        return None;
    };
    if arguments.args.len() == 1 {
        if let Some(class) = reflected_object_class(element) {
            return Some(quote!(<#class as ::godot_rs::engine::GodotClass>::CLASS_NAME));
        }
    }
    let name = (arguments.args.len() == 1)
        .then(|| reflected_array_element_name(element))
        .flatten()?;
    Some(quote!(#name))
}

fn reflected_array_element_name(value: &Type) -> Option<String> {
    if let Some(class_name) = reflected_object_class(value) {
        return outer_type_name(&class_name);
    }
    let Type::Path(path) = value else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let name = match segment.ident.to_string().as_str() {
        "bool" => "bool",
        "i32" | "i64" => "int",
        "f32" | "f64" => "float",
        "Aabb" => "AABB",
        "Rid" => "RID",
        "String" | "StringName" | "NodePath" | "Variant" | "Array" | "Dictionary" | "Callable"
        | "Signal" | "Vector2" | "Vector2i" | "Vector3" | "Vector3i" | "Vector4" | "Vector4i"
        | "Rect2" | "Rect2i" | "Quaternion" | "Plane" | "Transform2D" | "Basis" | "Transform3D"
        | "Projection" | "Color" | "PackedByteArray" | "PackedInt32Array" | "PackedInt64Array"
        | "PackedFloat32Array" | "PackedFloat64Array" | "PackedStringArray"
        | "PackedVector2Array" | "PackedVector3Array" | "PackedColorArray"
        | "PackedVector4Array" => {
            return Some(segment.ident.to_string());
        }
        _ => return None,
    };
    Some(name.to_owned())
}

fn reflected_return_value_metadata(output: &ReturnType) -> Option<TokenStream2> {
    let ReturnType::Type(_, output_type) = output else {
        return None;
    };
    if !matches!(
        outer_type_name(output_type).as_deref(),
        Some("ScriptResult" | "EngineResult")
    ) {
        return reflected_value_metadata(output_type);
    }
    let Type::Path(path) = output_type.as_ref() else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(value_type) = arguments.args.first()? else {
        return None;
    };
    (arguments.args.len() == 1)
        .then(|| reflected_value_metadata(value_type))
        .flatten()
}

fn take_parameter_default(argument: &mut syn::PatType) -> syn::Result<Option<Expr>> {
    let mut found = None;
    let mut retained = Vec::with_capacity(argument.attrs.len());
    for attribute in core::mem::take(&mut argument.attrs) {
        if !attribute.path().is_ident("default") {
            retained.push(attribute);
            continue;
        }
        if found.is_some() {
            return Err(Error::new_spanned(
                attribute,
                "duplicate `#[default(...)]` argument attribute",
            ));
        }
        let Meta::List(list) = attribute.meta else {
            return Err(Error::new_spanned(
                attribute,
                "write method defaults as `#[default(expression)]`",
            ));
        };
        let expression = syn::parse2::<Expr>(list.tokens.clone()).map_err(|_| {
            Error::new_spanned(
                &list,
                "`#[default(...)]` requires exactly one valid Rust expression",
            )
        })?;
        found = Some(expression);
    }
    argument.attrs = retained;
    Ok(found)
}

fn is_variant_slice(value: &Type) -> bool {
    let Type::Reference(reference) = value else {
        return false;
    };
    if reference.mutability.is_some() {
        return false;
    }
    let Type::Slice(slice) = reference.elem.as_ref() else {
        return false;
    };
    outer_type_name(&slice.elem).as_deref() == Some("Variant") && !has_type_arguments(&slice.elem)
}

fn return_abi_value_type(output: &ReturnType) -> syn::Result<TokenStream2> {
    let ReturnType::Type(_, output_type) = output else {
        return Ok(quote!(::godot_rs::abi::AbiValueType::NIL));
    };
    if matches!(output_type.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty()) {
        return Ok(quote!(::godot_rs::abi::AbiValueType::NIL));
    }
    if !matches!(
        outer_type_name(output_type).as_deref(),
        Some("ScriptResult" | "EngineResult")
    ) {
        return abi_value_type(output_type);
    }
    let Type::Path(path) = output_type.as_ref() else {
        unreachable!("outer type name only succeeds for a path");
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(Error::new_spanned(
            output_type,
            "ScriptResult requires one value type",
        ));
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(Error::new_spanned(
            output_type,
            "ScriptResult requires one value type",
        ));
    };
    let Some(GenericArgument::Type(value_type)) = arguments.args.first() else {
        return Err(Error::new_spanned(
            output_type,
            "ScriptResult requires one value type",
        ));
    };
    if matches!(value_type, Type::Tuple(tuple) if tuple.elems.is_empty()) {
        Ok(quote!(::godot_rs::abi::AbiValueType::NIL))
    } else {
        abi_value_type(value_type)
    }
}

fn take_method_attribute(
    attributes: &mut Vec<Attribute>,
    name: &str,
) -> syn::Result<Option<Attribute>> {
    let mut found = None;
    let mut retained = Vec::with_capacity(attributes.len());
    for attribute in core::mem::take(attributes) {
        if attribute.path().is_ident(name) {
            if found.is_some() {
                return Err(Error::new_spanned(
                    attribute,
                    format!("duplicate `#[{name}]` attribute"),
                ));
            }
            found = Some(attribute);
        } else {
            retained.push(attribute);
        }
    }
    *attributes = retained;
    Ok(found)
}

#[derive(Clone)]
struct ParsedRpcConfig {
    any_peer: bool,
    call_local: bool,
    transfer: ParsedRpcTransfer,
    channel: u32,
    name: Option<String>,
}

#[derive(Clone, Copy)]
enum ParsedRpcTransfer {
    Unreliable,
    UnreliableOrdered,
    Reliable,
}

impl ParsedRpcConfig {
    fn tokens(&self) -> TokenStream2 {
        let mode = if self.any_peer {
            quote!(::godot_rs::script::RpcMode::AnyPeer)
        } else {
            quote!(::godot_rs::script::RpcMode::Authority)
        };
        let call_local = self.call_local;
        let transfer = match self.transfer {
            ParsedRpcTransfer::Unreliable => {
                quote!(::godot_rs::script::RpcTransferMode::Unreliable)
            }
            ParsedRpcTransfer::UnreliableOrdered => {
                quote!(::godot_rs::script::RpcTransferMode::UnreliableOrdered)
            }
            ParsedRpcTransfer::Reliable => {
                quote!(::godot_rs::script::RpcTransferMode::Reliable)
            }
        };
        let channel = self.channel;
        quote! {
            ::core::option::Option::Some(::godot_rs::script::RpcConfig {
                mode: #mode,
                call_local: #call_local,
                transfer_mode: #transfer,
                channel: #channel,
            })
        }
    }
}

fn parse_rpc_arguments(attribute: &Attribute) -> syn::Result<ParsedRpcConfig> {
    let entries = parse_meta_arguments(attribute)?;
    let mut authority = false;
    let mut any_peer = false;
    let mut call_remote = false;
    let mut call_local = false;
    let mut transfer = None::<&'static str>;
    let mut channel = None;
    let mut name = None;
    for entry in entries {
        match entry {
            Meta::Path(path) if path.is_ident("authority") => authority = true,
            Meta::Path(path) if path.is_ident("any_peer") => any_peer = true,
            Meta::Path(path) if path.is_ident("call_remote") => call_remote = true,
            Meta::Path(path) if path.is_ident("call_local") => call_local = true,
            Meta::Path(path) if path.is_ident("reliable") => {
                set_rpc_transfer(&mut transfer, "reliable", &path)?;
            }
            Meta::Path(path) if path.is_ident("unreliable") => {
                set_rpc_transfer(&mut transfer, "unreliable", &path)?;
            }
            Meta::Path(path) if path.is_ident("unreliable_ordered") => {
                set_rpc_transfer(&mut transfer, "unreliable_ordered", &path)?;
            }
            Meta::NameValue(value) if value.path.is_ident("channel") => {
                if channel.is_some() {
                    return Err(Error::new_spanned(
                        value,
                        "declare the RPC channel only once",
                    ));
                }
                let Expr::Lit(expression) = &value.value else {
                    return Err(Error::new_spanned(
                        &value.value,
                        "RPC channel must be an integer literal",
                    ));
                };
                let syn::Lit::Int(integer) = &expression.lit else {
                    return Err(Error::new_spanned(
                        &expression.lit,
                        "RPC channel must be an integer literal",
                    ));
                };
                channel = Some(integer.base10_parse::<u32>().map_err(|_| {
                    Error::new_spanned(integer, "RPC channel must fit an unsigned 32-bit integer")
                })?);
            }
            Meta::NameValue(value) if value.path.is_ident("name") => {
                set_reflected_name(&mut name, value, "RPC")?;
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "unsupported RPC option; use `name`, `authority`, `any_peer`, `call_local`, `call_remote`, a transfer mode, or `channel = integer`",
                ));
            }
        }
    }
    if authority && any_peer {
        return Err(Error::new_spanned(
            attribute,
            "choose either `authority` or `any_peer`, not both",
        ));
    }
    if call_remote && call_local {
        return Err(Error::new_spanned(
            attribute,
            "choose either `call_remote` or `call_local`, not both",
        ));
    }
    let transfer = match transfer {
        None | Some("unreliable") => ParsedRpcTransfer::Unreliable,
        Some("unreliable_ordered") => ParsedRpcTransfer::UnreliableOrdered,
        Some("reliable") => ParsedRpcTransfer::Reliable,
        Some(_) => unreachable!("RPC transfer parser only stores supported values"),
    };
    Ok(ParsedRpcConfig {
        any_peer,
        call_local,
        transfer,
        channel: channel.unwrap_or(0),
        name,
    })
}

fn parse_func_arguments(attribute: &Attribute) -> syn::Result<Option<String>> {
    if matches!(attribute.meta, Meta::Path(_)) {
        return Ok(None);
    }
    let entries = parse_meta_arguments(attribute)?;
    let mut name = None;
    for entry in entries {
        match entry {
            Meta::NameValue(value) if value.path.is_ident("name") => {
                set_reflected_name(&mut name, value, "function")?;
            }
            unsupported => {
                return Err(Error::new_spanned(
                    unsupported,
                    "the only supported function option is `name = \"godot_name\"`",
                ));
            }
        }
    }
    Ok(name)
}

fn set_reflected_name(
    current: &mut Option<String>,
    value: syn::MetaNameValue,
    kind: &str,
) -> syn::Result<()> {
    if current.is_some() {
        return Err(Error::new_spanned(
            value,
            format!("declare the {kind} name only once"),
        ));
    }
    let Expr::Lit(expression) = &value.value else {
        return Err(Error::new_spanned(
            &value.value,
            format!("{kind} name must be a string literal"),
        ));
    };
    let syn::Lit::Str(name) = &expression.lit else {
        return Err(Error::new_spanned(
            &expression.lit,
            format!("{kind} name must be a string literal"),
        ));
    };
    let name = name.value();
    if !is_godot_class_identifier(&name) {
        return Err(Error::new_spanned(
            expression,
            format!("{kind} name must be a valid Godot identifier"),
        ));
    }
    *current = Some(name);
    Ok(())
}

fn set_rpc_transfer(
    current: &mut Option<&'static str>,
    next: &'static str,
    source: &Path,
) -> syn::Result<()> {
    if current.replace(next).is_some() {
        return Err(Error::new_spanned(
            source,
            "choose only one RPC transfer mode",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Receiver {
    Shared,
    Mutable,
    Static,
}

fn receiver_kind(method: &ImplItemFn) -> syn::Result<Receiver> {
    let Some(first) = method.sig.inputs.first() else {
        return Ok(Receiver::Static);
    };
    let FnArg::Receiver(receiver) = first else {
        return Ok(Receiver::Static);
    };
    if receiver.reference.is_none() {
        return Err(Error::new_spanned(
            receiver,
            "Godot-visible methods must borrow `self` instead of consuming it",
        ));
    }
    Ok(if receiver.mutability.is_some() {
        Receiver::Mutable
    } else {
        Receiver::Shared
    })
}

#[derive(Clone, Copy)]
enum Lifecycle {
    EnterTree,
    Ready,
    Process,
    PhysicsProcess,
    Input,
    UnhandledInput,
    ExitTree,
}

impl Lifecycle {
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "_enter_tree" => Some(Self::EnterTree),
            "_ready" => Some(Self::Ready),
            "_process" => Some(Self::Process),
            "_physics_process" => Some(Self::PhysicsProcess),
            "_input" => Some(Self::Input),
            "_unhandled_input" => Some(Self::UnhandledInput),
            "_exit_tree" => Some(Self::ExitTree),
            _ => None,
        }
    }

    fn validate(self, method: &ImplItemFn) -> syn::Result<()> {
        if receiver_kind(method)? != Receiver::Mutable {
            return Err(Error::new_spanned(
                &method.sig,
                format!("`{}` must start with `&mut self`", method.sig.ident),
            ));
        }
        let arguments: Vec<_> = method
            .sig
            .inputs
            .iter()
            .filter_map(|argument| match argument {
                FnArg::Typed(argument) => Some(argument),
                FnArg::Receiver(_) => None,
            })
            .collect();
        match self {
            Self::EnterTree | Self::Ready | Self::ExitTree if arguments.is_empty() => {}
            Self::Process | Self::PhysicsProcess
                if arguments.len() == 1 && type_is(&arguments[0].ty, "f64") => {}
            Self::Input | Self::UnhandledInput
                if arguments.len() == 1 && type_is(&arguments[0].ty, "InputEventRef") => {}
            _ => {
                return Err(Error::new_spanned(&method.sig, self.expected_signature()));
            }
        }
        if !valid_callback_return(&method.sig.output) {
            return Err(Error::new_spanned(
                &method.sig.output,
                "lifecycle callbacks must return `()`, `ScriptResult<()>`, or `EngineResult<()>`",
            ));
        }
        Ok(())
    }

    fn expected_signature(self) -> &'static str {
        match self {
            Self::EnterTree => "expected `fn _enter_tree(&mut self)`",
            Self::Ready => "expected `fn _ready(&mut self)`",
            Self::Process => "expected `fn _process(&mut self, delta: f64)`",
            Self::PhysicsProcess => "expected `fn _physics_process(&mut self, delta: f64)`",
            Self::Input => "expected `fn _input(&mut self, event: InputEventRef)`",
            Self::UnhandledInput => {
                "expected `fn _unhandled_input(&mut self, event: InputEventRef)`"
            }
            Self::ExitTree => "expected `fn _exit_tree(&mut self)`",
        }
    }

    fn kind_tokens(self) -> TokenStream2 {
        let slot = match self {
            Self::EnterTree => quote!(::godot_rs::script::LifecycleSlot::EnterTree),
            Self::Ready => quote!(::godot_rs::script::LifecycleSlot::Ready),
            Self::Process => quote!(::godot_rs::script::LifecycleSlot::Process),
            Self::PhysicsProcess => {
                quote!(::godot_rs::script::LifecycleSlot::PhysicsProcess)
            }
            Self::Input => quote!(::godot_rs::script::LifecycleSlot::Input),
            Self::UnhandledInput => {
                quote!(::godot_rs::script::LifecycleSlot::UnhandledInput)
            }
            Self::ExitTree => quote!(::godot_rs::script::LifecycleSlot::ExitTree),
        };
        quote!(::godot_rs::script::MethodKind::Lifecycle(#slot))
    }

    fn argument_types(self) -> Vec<TokenStream2> {
        match self {
            Self::EnterTree | Self::Ready | Self::ExitTree => Vec::new(),
            Self::Process | Self::PhysicsProcess => {
                vec![quote!(::godot_rs::abi::AbiValueType::F64)]
            }
            Self::Input | Self::UnhandledInput => {
                vec![quote!(::godot_rs::abi::AbiValueType::OBJECT_ID)]
            }
        }
    }

    fn wrapper(
        self,
        self_type: &Type,
        method: &syn::Ident,
        wrapper: &syn::Ident,
    ) -> (TokenStream2, TokenStream2) {
        let convert =
            quote!(::godot_rs::script::IntoCallbackStatus::into_callback_status(result).into_abi());
        let reject_null = quote! {
            fn assert_node_callback<T: ::godot_rs::script::ScriptBase>()
            where
                T::Base: ::godot_rs::engine::Inherits<::godot_rs::engine::Node>,
            {}
            assert_node_callback::<#self_type>();
            if instance.is_null() {
                return ::godot_rs::abi::AbiCallResult::failure(
                    ::godot_rs::abi::AbiStatus::InvalidArgument,
                    "script callback received null state",
                );
            }
        };
        match self {
            Self::EnterTree | Self::Ready | Self::ExitTree => (
                quote!(::godot_rs::script::MethodCallback::Lifecycle0(#wrapper)),
                quote! {
                    unsafe extern "C" fn #wrapper(
                        instance: *mut ::core::ffi::c_void,
                    ) -> ::godot_rs::abi::AbiCallResult {
                        #reject_null
                        ::godot_rs::script::catch_abi_panic(|| {
                            // SAFETY: The module runtime supplies the matching live script state.
                            let instance = unsafe { &mut *instance.cast::<#self_type>() };
                            let result = instance.#method();
                            #convert
                        })
                    }
                },
            ),
            Self::Process | Self::PhysicsProcess => (
                quote!(::godot_rs::script::MethodCallback::LifecycleF64(#wrapper)),
                quote! {
                    unsafe extern "C" fn #wrapper(
                        instance: *mut ::core::ffi::c_void,
                        delta: f64,
                    ) -> ::godot_rs::abi::AbiCallResult {
                        #reject_null
                        ::godot_rs::script::catch_abi_panic(|| {
                            // SAFETY: The module runtime supplies the matching live script state.
                            let instance = unsafe { &mut *instance.cast::<#self_type>() };
                            let result = instance.#method(delta);
                            #convert
                        })
                    }
                },
            ),
            Self::Input | Self::UnhandledInput => (
                quote!(::godot_rs::script::MethodCallback::LifecycleInput(#wrapper)),
                quote! {
                    unsafe extern "C" fn #wrapper(
                        instance: *mut ::core::ffi::c_void,
                        event: u64,
                    ) -> ::godot_rs::abi::AbiCallResult {
                        #reject_null
                        ::godot_rs::script::catch_abi_panic(|| {
                            // SAFETY: The module runtime supplies the matching live script state.
                            let instance = unsafe { &mut *instance.cast::<#self_type>() };
                            let event = ::godot_rs::engine::InputEventRef::from_raw(event);
                            let result = instance.#method(event);
                            #convert
                        })
                    }
                },
            ),
        }
    }
}

fn valid_callback_return(output: &ReturnType) -> bool {
    match output {
        ReturnType::Default => true,
        ReturnType::Type(_, output) => match output.as_ref() {
            Type::Tuple(tuple) => tuple.elems.is_empty(),
            Type::Path(_) => single_type_argument(output, "ScriptResult")
                .or_else(|| single_type_argument(output, "EngineResult"))
                .is_some_and(|value| matches!(value, Type::Tuple(tuple) if tuple.elems.is_empty())),
            _ => false,
        },
    }
}

fn type_is(value: &Type, expected: &str) -> bool {
    outer_type_name(value).as_deref() == Some(expected)
}

fn self_type_name(self_type: &Type) -> syn::Result<syn::Ident> {
    let Type::Path(path) = self_type else {
        return Err(Error::new_spanned(
            self_type,
            "script impl target must be a named script type",
        ));
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.clone())
        .ok_or_else(|| Error::new_spanned(self_type, "script impl target has no type name"))
}

fn attribute_options(attribute: &Attribute) -> String {
    match &attribute.meta {
        Meta::Path(_) => String::new(),
        Meta::List(list) => list.tokens.to_string(),
        Meta::NameValue(value) => value.value.to_token_stream().to_string(),
    }
}

fn token_string(tokens: &impl ToTokens) -> LitStr {
    LitStr::new(&tokens.to_token_stream().to_string(), Span::call_site())
}

const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn script_arguments_require_supported_shape() {
        let arguments: ScriptArguments = syn::parse2(quote!(
            base = Node2D,
            class_name = Player,
            extends = "res://scripts/base.rs",
            icon = "res://icons/player.svg",
            tool,
            abstract
        ))
        .expect("valid script arguments");
        assert_eq!(
            arguments
                .base
                .expect("base")
                .segments
                .last()
                .expect("segment")
                .ident,
            "Node2D"
        );
        assert!(arguments.tool);
        assert!(arguments.abstract_);
        assert_eq!(arguments.class_name.as_deref(), Some("Player"));
        assert_eq!(arguments.extends.as_deref(), Some("res://scripts/base.rs"));
        assert_eq!(arguments.icon.as_deref(), Some("res://icons/player.svg"));

        let error = syn::parse2::<ScriptArguments>(quote!(godot = "4.4"))
            .err()
            .expect("unknown option must fail");
        assert!(error.to_string().contains("supported script arguments"));
        let error =
            syn::parse2::<ScriptArguments>(quote!(base = Node, extends = "res://scripts//base.rs"))
                .err()
                .expect("noncanonical parent path must fail");
        assert!(error.to_string().contains("canonical"));
        let error =
            syn::parse2::<ScriptArguments>(quote!(base = Node, icon = "../icons/player.svg"))
                .err()
                .expect("non-resource icon path must fail");
        assert!(error.to_string().contains("canonical"));
    }

    #[test]
    fn lifecycle_signatures_are_checked() {
        let valid: ImplItemFn = parse_quote!(
            fn _physics_process(&mut self, delta: f64) {
                let _ = delta;
            }
        );
        Lifecycle::PhysicsProcess
            .validate(&valid)
            .expect("valid callback");

        let invalid: ImplItemFn = parse_quote!(
            fn _physics_process(&self, delta: f32) {}
        );
        let error = Lifecycle::PhysicsProcess
            .validate(&invalid)
            .expect_err("invalid callback");
        assert!(error.to_string().contains("&mut self"));

        let invalid: ImplItemFn = parse_quote!(
            fn _ready(&mut self) -> EngineResult<i64> {
                Ok(1)
            }
        );
        assert!(
            Lifecycle::Ready
                .validate(&invalid)
                .expect_err("callbacks cannot return values")
                .to_string()
                .contains("EngineResult<()>")
        );
    }

    #[test]
    fn node_fields_enforce_optional_shape() {
        let required: Type = parse_quote!(NodeRef<Sprite2D>);
        assert_eq!(
            outer_type_name(
                &validate_node_type(&required, false, &parse_quote!(#[node("%Sprite")]))
                    .expect("required NodeRef")
            )
            .as_deref(),
            Some("Sprite2D")
        );

        let optional: Type = parse_quote!(Option<NodeRef<Camera2D>>);
        assert_eq!(
            outer_type_name(
                &validate_node_type(&optional, true, &parse_quote!(#[node("%Camera", optional)]))
                    .expect("optional NodeRef")
            )
            .as_deref(),
            Some("Camera2D")
        );

        let invalid: Type = parse_quote!(NodeRef<Camera2D>);
        let error = validate_node_type(&invalid, true, &parse_quote!(#[node("%Camera", optional)]))
            .map(|_| ())
            .expect_err("optional field must use Option");
        assert!(error.to_string().contains("Option<NodeRef<T>>"));

        let mut attributes = vec![parse_quote!(#[node("%Camera")])];
        let inferred = take_field_metadata(&mut attributes, &optional).expect("inferred Option");
        assert!(inferred.node_optional);
        assert!(attributes.is_empty());
    }

    #[test]
    fn field_initialization_is_independent_of_reload() {
        let mut attributes = vec![
            parse_quote!(#[init(default = Vec::with_capacity(16))]),
            parse_quote!(#[reload(skip)]),
        ];
        let metadata = take_field_metadata(&mut attributes, &parse_quote!(Vec<i64>))
            .expect("plain state initializer");
        assert!(metadata.default.is_some());
        assert!(metadata.reload == ReloadPolicy::Skip);
        assert!(attributes.is_empty());

        for attributes in [
            vec![parse_quote!(#[init])],
            vec![parse_quote!(#[export(default = 1, default = 2)])],
            vec![
                parse_quote!(#[init(default = 1)]),
                parse_quote!(#[init(default = 2)]),
            ],
            vec![parse_quote!(#[export]), parse_quote!(#[init(default = 1)])],
            vec![parse_quote!(#[init(default = 1)]), parse_quote!(#[export])],
        ] {
            assert!(take_field_metadata(&mut attributes.clone(), &parse_quote!(i64)).is_err());
        }
        for mut attributes in [
            vec![
                parse_quote!(#[export]),
                parse_quote!(#[reload(default = Array::default())]),
            ],
            vec![
                parse_quote!(#[reload(default = Array::default())]),
                parse_quote!(#[export]),
            ],
        ] {
            assert!(take_field_metadata(&mut attributes, &parse_quote!(Array<i64>)).is_err());
        }
    }

    #[test]
    fn function_signals_keep_names_and_types_together() {
        let schema = parse_signal_schema(
            &parse_quote!(#[signal]),
            &parse_quote!(Signal<fn(previous: i64, current: i64)>),
        )
        .expect("named function signature");
        assert_eq!(schema.len(), 2);
        assert_eq!(schema[0].name, "previous");
        assert_eq!(schema[1].name, "current");
        assert!(
            parse_signal_schema(&parse_quote!(#[signal]), &parse_quote!(Signal<fn()>))
                .expect("zero argument signal")
                .is_empty()
        );

        for signature in [
            parse_quote!(Signal<fn(i64)>),
            parse_quote!(Signal<fn(value: i64, value: i64)>),
            parse_quote!(Signal<fn(value: i64) -> bool>),
            parse_quote!(Signal<unsafe fn(value: i64)>),
        ] {
            assert!(parse_signal_schema(&parse_quote!(#[signal]), &signature).is_err());
        }
        assert!(
            parse_signal_schema(
                &parse_quote!(#[signal(args(value))]),
                &parse_quote!(Signal<fn(value: i64)>),
            )
            .is_err()
        );
    }

    #[test]
    fn unsupported_callbacks_do_not_silently_become_helpers() {
        let methods: [ImplItemFn; 2] = [
            parse_quote!(
                fn _init(&mut self) {}
            ),
            parse_quote!(
                fn _notification(&mut self, what: i32) {}
            ),
        ];
        for mut method in methods {
            assert!(
                method_descriptor(
                    &parse_quote!(Player),
                    &parse_quote!(Player),
                    &mut method,
                    None
                )
                .is_err()
            );
        }
        let mut helper = parse_quote!(
            fn _private_helper(&self) {}
        );
        assert!(
            method_descriptor(
                &parse_quote!(Player),
                &parse_quote!(Player),
                &mut helper,
                None
            )
            .expect("ordinary Rust helper")
            .is_none()
        );
    }

    #[test]
    fn method_ids_are_stable_and_distinct() {
        assert_eq!(fnv1a(b"_ready"), 723_962_071_783_188_397);
        assert_ne!(fnv1a(b"_ready"), fnv1a(b"_process"));
    }

    #[test]
    fn custom_integer_names_use_godot_style_options() {
        assert_eq!(screaming_snake_case("VeryFast"), "VERY_FAST");
        assert_eq!(screaming_snake_case("Idle2D"), "IDLE2_D");
    }
}
