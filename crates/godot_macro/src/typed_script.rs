use super::*;

pub(super) fn api_name(name: &Ident) -> Ident {
    format_ident!("__Godot{}Api", name)
}

fn callbacks_name(name: &Ident) -> Ident {
    format_ident!("__Godot{}Callbacks", name)
}

pub(super) fn class_api(
    name: &Ident,
    visibility: &Visibility,
    base: &Ident,
    property_methods: &[TokenStream2],
    signal_methods: &[TokenStream2],
) -> TokenStream2 {
    let api = api_name(name);
    let callbacks = callbacks_name(name);
    let properties = format_ident!("__Godot{}Properties", name);
    let signals = format_ident!("__Godot{}Signals", name);
    quote! {
        #[doc(hidden)]
        #[repr(transparent)]
        #visibility struct #api {
            object: ::godot_rs::engine::ObjectRef<#name>,
        }

        // SAFETY: The generated proxy has exactly the ObjectRef<T> representation.
        unsafe impl ::godot_rs::script::ScriptApi<#name> for #api {}

        #[doc(hidden)]
        #visibility struct #callbacks {
            object: ::godot_rs::engine::ObjectRef<#name>,
        }

        #[doc(hidden)]
        #visibility struct #properties {
            object: ::godot_rs::engine::ObjectRef<#name>,
        }

        impl #properties {
            #(#property_methods)*
        }

        #[doc(hidden)]
        #visibility struct #signals {
            object: ::godot_rs::engine::ObjectRef<#name>,
        }

        impl #signals {
            #(#signal_methods)*
        }

        impl #api {
            /// Returns typed targets for this script's Godot-visible methods.
            pub fn callbacks(&self) -> #callbacks {
                #callbacks { object: self.object }
            }

            /// Returns typed access to this script's Godot properties.
            pub fn properties(&self) -> #properties {
                #properties { object: self.object }
            }

            /// Returns this script's signals with their declared argument types.
            pub fn signals(&self) -> #signals {
                #signals { object: self.object }
            }
        }

        impl #name {
            /// Returns tasks that are cancelled when this script state is destroyed or replaced.
            pub fn tasks(&self) -> &::godot_rs::task::TaskScope {
                self.__godot_rs_tasks.get_or_init(::core::default::Default::default)
            }

            /// Returns typed signal connection targets for the current script instance.
            pub fn callbacks(&self) -> ::godot_rs::error::EngineResult<#callbacks> {
                ::godot_rs::engine::current_object::<Self>()
                    .map(|object| #callbacks { object })
            }
        }

        impl ::godot_rs::engine::GodotClass for #name {
            const CLASS_NAME: &'static str =
                <::godot_rs::engine::#base as ::godot_rs::engine::GodotClass>::CLASS_NAME;
            const SCRIPT_ID: ::core::option::Option<&'static str> =
                ::core::option::Option::Some(concat!(module_path!(), "::", stringify!(#name)));

            fn __script_resource_uid() -> ::godot_rs::error::EngineResult<::core::option::Option<i64>> {
                static UID: ::std::sync::OnceLock<i64> = ::std::sync::OnceLock::new();
                if let ::core::option::Option::Some(uid) = UID.get() {
                    return ::core::result::Result::Ok(::core::option::Option::Some(*uid));
                }
                let uid = ::godot_rs::script::__registered_script_uid(
                    concat!(module_path!(), "::", stringify!(#name)),
                )?;
                let _ = UID.set(uid);
                ::core::result::Result::Ok(::core::option::Option::Some(uid))
            }
        }

        ::godot_rs::godot_rs_inherit_script_base!(#name, #base);
    }
}

pub(super) fn method_api(
    self_type: &Type,
    name: &Ident,
    method: &ImplItemFn,
    method_id: u64,
    godot_name: &LitStr,
    vararg: bool,
) -> syn::Result<TokenStream2> {
    if vararg {
        return Ok(TokenStream2::new());
    }
    let api = api_name(name);
    let callbacks = callbacks_name(name);
    let method_name = &method.sig.ident;
    if method_name == "callbacks"
        || method_name == "properties"
        || method_name == "signals"
        || method_name == "tasks"
    {
        return Err(Error::new_spanned(
            method_name,
            "`callbacks`, `properties`, `signals`, and `tasks` are reserved for typed script access",
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
    let argument_types: Vec<_> = arguments
        .iter()
        .map(|argument| argument.ty.as_ref())
        .collect();
    let argument_names: Vec<_> = arguments
        .iter()
        .map(|argument| match argument.pat.as_ref() {
            Pat::Ident(pattern) => &pattern.ident,
            _ => unreachable!("method argument names were validated"),
        })
        .collect();
    let argument_values: Vec<_> = argument_types
        .iter()
        .zip(&argument_names)
        .map(|(ty, name)| abi_argument(ty, name))
        .collect();
    let return_type = match &method.sig.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) => single_type_argument(ty, "ScriptResult")
            .or_else(|| single_type_argument(ty, "EngineResult"))
            .unwrap_or(ty)
            .clone(),
    };
    let documentation: Vec<_> = method
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .collect();
    Ok(quote! {
        impl #api {
            #(#documentation)*
            pub fn #method_name(&self, #(#argument_names: #argument_types),*)
                -> ::godot_rs::error::EngineResult<#return_type>
            {
                let arguments = [#(#argument_values),*];
                ::godot_rs::script::__call_script_method::<#self_type, #return_type>(
                    self.object, #method_id, &arguments,
                )
            }
        }

        impl #callbacks {
            #(#documentation)*
            pub fn #method_name(&self)
                -> ::godot_rs::callable::CallableMethod<(#(#argument_types,)*)>
            {
                ::godot_rs::callable::CallableMethod::__new(
                    ::godot_rs::callable::Callable::from_object_method(self.object, #godot_name),
                )
            }
        }
    })
}

fn abi_argument(ty: &Type, name: &Ident) -> TokenStream2 {
    if matches!(
        outer_type_name(ty).as_deref(),
        Some("NodePath" | "StringName")
    ) {
        let abi_type = if outer_type_name(ty).as_deref() == Some("NodePath") {
            quote!(::godot_rs::abi::AbiValueType::NODE_PATH)
        } else {
            quote!(::godot_rs::abi::AbiValueType::STRING_NAME)
        };
        return quote!(::godot_rs::abi::AbiValueV1::from_borrowed_bytes(
            #abi_type, #name.as_str().as_bytes(),
        ));
    }
    if single_type_argument(ty, "Option")
        .and_then(|ty| single_type_argument(ty, "GodotRef"))
        .is_some()
    {
        return quote! {
            match #name.as_ref() {
                ::core::option::Option::Some(value) =>
                    ::godot_rs::engine::EngineArgument::__into_engine_argument(value),
                ::core::option::Option::None => ::godot_rs::abi::AbiValueV1::from_object_id(0),
            }
        };
    }
    let value = match outer_type_name(ty).as_deref() {
        Some("String") => quote!(#name.as_str()),
        Some(
            "Variant" | "Array" | "Dictionary" | "Callable" | "Signal" | "GodotRef" | "Transform2D"
            | "Aabb" | "Basis" | "Transform3D" | "Projection" | "PackedByteArray"
            | "PackedInt32Array" | "PackedInt64Array" | "PackedFloat32Array" | "PackedFloat64Array"
            | "PackedStringArray" | "PackedVector2Array" | "PackedVector3Array"
            | "PackedColorArray" | "PackedVector4Array",
        ) => quote!(&#name),
        _ => quote!(#name),
    };
    quote!(::godot_rs::engine::EngineArgument::__into_engine_argument(#value))
}

pub(super) fn property_api(name: &Ident, ty: &Type, index: u32, read_only: bool) -> TokenStream2 {
    let setter = if read_only {
        TokenStream2::new()
    } else {
        let setter_name = format_ident!("set_{}", name);
        let value = format_ident!("value");
        let argument = abi_argument(ty, &value);
        quote! {
            pub fn #setter_name(&self, #value: #ty) -> ::godot_rs::error::EngineResult<()> {
                ::godot_rs::script::__set_script_property(self.object, #index, #argument)
            }
        }
    };
    quote! {
        pub fn #name(&self) -> ::godot_rs::error::EngineResult<#ty> {
            ::godot_rs::script::__get_script_property(self.object, #index)
        }
        #setter
    }
}

pub(super) fn signal_api(name: &Ident, ty: &Type) -> TokenStream2 {
    let signal_name = godot_identifier(name);
    quote! {
        pub fn #name(&self) -> #ty {
            ::godot_rs::signal::Signal::__from_script(self.object, #signal_name)
        }
    }
}
