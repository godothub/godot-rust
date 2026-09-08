use super::*;

/// Retains container arguments while a receiving script copies or stores them.
pub(crate) struct ScriptCallArguments<'a> {
    context: &'a EngineCallContext,
    original: &'a [AbiValueV1],
    converted: Option<Vec<AbiValueV1>>,
    owned: Vec<AbiValueV1>,
}

impl<'a> ScriptCallArguments<'a> {
    pub(crate) fn new(
        context: &'a EngineCallContext,
        interface: EngineInterface,
        arguments: &'a [AbiValueV1],
    ) -> Result<Self, ValueError> {
        let mut result = Self {
            context,
            original: arguments,
            converted: None,
            owned: Vec::new(),
        };
        for (index, value) in arguments.iter().copied().enumerate() {
            if !matches!(
                value.type_,
                AbiValueType::VARIANT
                    | AbiValueType::ARRAY
                    | AbiValueType::DICTIONARY
                    | AbiValueType::CALLABLE
            ) {
                continue;
            }
            let retained = if value.type_ == AbiValueType::CALLABLE {
                copy_script_callable(context, interface, value, context)?
            } else {
                let native =
                    NativeDynamic::from_abi(interface, value.type_, value, None, Some(context))?;
                own_returned_dynamic(context, value.type_, native)?
            };
            result.owned.push(retained);
            let mut borrowed = retained;
            borrowed.reserved_flags = 0;
            result.converted.get_or_insert_with(|| arguments.to_vec())[index] = borrowed;
        }
        Ok(result)
    }

    pub(crate) fn as_slice(&self) -> &[AbiValueV1] {
        self.converted.as_deref().unwrap_or(self.original)
    }
}

impl Drop for ScriptCallArguments<'_> {
    fn drop(&mut self) {
        for value in self.owned.drain(..) {
            let _ = drop_host_value(ptr::from_ref(self.context).cast_mut().cast(), value);
        }
    }
}
