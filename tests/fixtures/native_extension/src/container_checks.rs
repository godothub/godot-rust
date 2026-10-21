use godot_rs::prelude::*;
use godot_rs::variant::VariantConvert;

pub fn verify(
    numbers: Array<i64>,
    dictionary: Dictionary,
    readonly: Array,
    typed_dictionary: Dictionary,
) -> EngineResult<bool> {
    let shared = numbers.clone();
    shared.set(0, 10)?;
    shared.push(3)?;
    if numbers.to_vec()? != [10, 2, 3] {
        return Ok(false);
    }
    let independent = numbers.duplicate(false)?;
    independent.push(100)?;
    if numbers.len()? != 3 || independent.is_same(&numbers)? {
        return Ok(false);
    }
    let mut snapshot = numbers.snapshot()?.into_vec();
    snapshot[0] = 99;
    if numbers.get(0)? != Some(10) {
        return Ok(false);
    }
    numbers.assign_from_slice(&[10, 20])?;

    let untyped = numbers.to_untyped();
    if untyped.push(Variant::from("wrong type")).is_ok() || numbers.len()? != 2 {
        return Ok(false);
    }
    if untyped.try_as::<String>().is_ok() {
        return Ok(false);
    }
    let value = Variant::from(numbers.clone());
    let Some(restored) = Array::<i64>::from_variant(value) else {
        return Ok(false);
    };
    if !restored.is_same(&numbers)? {
        return Ok(false);
    }

    let aliases = Array::<Variant>::from_vec(vec![numbers.to_variant(), numbers.to_variant()])?;
    let first = aliases.get(0)?.expect("first alias");
    let second = aliases.get(1)?.expect("second alias");
    let (VariantKind::Array(first), VariantKind::Array(second)) = (first.kind(), second.kind())
    else {
        return Ok(false);
    };
    if !first.is_same(second)? || !first.is_same(&numbers)? {
        return Ok(false);
    }
    let shallow = aliases.duplicate(false)?;
    let shallow_child = shallow.get(0)?.expect("shallow child");
    let deep = aliases.duplicate(true)?;
    let deep_child = deep.get(0)?.expect("deep child");
    let (VariantKind::Array(shallow_child), VariantKind::Array(deep_child)) =
        (shallow_child.kind(), deep_child.kind())
    else {
        return Ok(false);
    };
    if !shallow_child.is_same(&numbers)? || deep_child.is_same(&numbers)? {
        return Ok(false);
    }
    deep_child.push(Variant::from(30_i64))?;
    if numbers.len()? != 2 {
        return Ok(false);
    }
    let cycle = Array::<Variant>::new();
    cycle.push(cycle.to_variant())?;
    let back = cycle.get(0)?.expect("cycle");
    let VariantKind::Array(back) = back.kind() else {
        return Ok(false);
    };
    let same_cycle = back.is_same(&cycle)?;
    cycle.clear()?; // Godot reference-counted container cycles must be broken.
    if !same_cycle {
        return Ok(false);
    }

    let shared_dictionary = dictionary.clone();
    shared_dictionary.insert("rust", 42_i64)?;
    let snapshot = dictionary.snapshot()?;
    shared_dictionary.insert("rust", 43_i64)?;
    if dictionary.get(&Variant::from("rust"))? != Some(Variant::from(43_i64))
        || !snapshot
            .as_entries()
            .iter()
            .any(|(key, value)| key == &Variant::from("rust") && value == &Variant::from(42_i64))
    {
        return Ok(false);
    }
    let independent_dictionary = dictionary.duplicate(false)?;
    independent_dictionary.insert("copy_only", true)?;
    if dictionary.get(&Variant::from("copy_only"))?.is_some() {
        return Ok(false);
    }
    shared_dictionary.insert("nil", Variant::nil())?;
    if dictionary.get(&Variant::from("nil"))? != Some(Variant::nil())
        || dictionary.get(&Variant::from("missing"))?.is_some()
    {
        return Ok(false);
    }

    if readonly.push(Variant::from(1_i64)).is_ok()
        || godot_rs::engine::builtin::array::append(&mut readonly.clone(), &Variant::from(1_i64))
            .is_ok()
        || readonly.assign_from_slice(&[Variant::from(1_i64)]).is_ok()
    {
        return Ok(false);
    }
    typed_dictionary.insert("valid", 5_i64)?;
    if typed_dictionary.insert("invalid", true).is_ok()
        || typed_dictionary
            .assign_from_entries(&[(Variant::from("invalid"), Variant::from(true))])
            .is_ok()
        || typed_dictionary.get(&Variant::from("valid"))? != Some(Variant::from(5_i64))
    {
        return Ok(false);
    }
    let packed = PackedInt64Array::from_vec(vec![1, 3, 5]);
    let from_packed = Array::<i64>::from_packed(&packed)?;
    if from_packed.to_packed()? != packed {
        return Ok(false);
    }

    let large = Dictionary::from_entries(
        (0..16_000_i64)
            .map(|i| (Variant::from(i), Variant::from(i)))
            .collect(),
    )?;
    let clone = large.clone();
    if !clone.is_same(&large)?
        || clone.__bytes().map(|bytes| bytes.len()).ok() != Some(44)
        || clone.get(&Variant::from(15_999_i64))? != Some(Variant::from(15_999_i64))
    {
        return Ok(false);
    }
    Ok(true)
}
