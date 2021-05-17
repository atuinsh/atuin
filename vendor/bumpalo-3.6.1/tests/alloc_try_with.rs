// All of these alloc_try_with tests will fail with "fatal runtime error: stack overflow" unless
// LLVM manages to optimize the stack writes away.
//
// We only run them when debug_assertions are not set, as we expect them to fail outside release
// mode.

use bumpalo::Bump;

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_array() -> Result<(), ()> {
    let b = Bump::new();

    b.alloc_try_with(|| Ok([4u8; 10_000_000]))?;

    Ok(())
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_array_err() {
    let b = Bump::new();

    assert!(b
        .alloc_try_with(|| Result::<[u8; 10_000_000], _>::Err(()))
        .is_err());
}

#[allow(dead_code)]
struct LargeStruct {
    small: usize,
    big1: [u8; 20_000_000],
    big2: [u8; 20_000_000],
    big3: [u8; 20_000_000],
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_struct() -> Result<(), ()> {
    let b = Bump::new();

    b.alloc_try_with(|| {
        Ok(LargeStruct {
            small: 1,
            big1: [2; 20_000_000],
            big2: [3; 20_000_000],
            big3: [4; 20_000_000],
        })
    })?;

    Ok(())
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_struct_err() {
    let b = Bump::new();

    assert!(b
        .alloc_try_with(|| Result::<LargeStruct, _>::Err(()))
        .is_err());
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_tuple() -> Result<(), ()> {
    let b = Bump::new();

    b.alloc_try_with(|| {
        Ok((
            1u32,
            LargeStruct {
                small: 2,
                big1: [3; 20_000_000],
                big2: [4; 20_000_000],
                big3: [5; 20_000_000],
            },
        ))
    })?;

    Ok(())
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_tuple_err() {
    let b = Bump::new();

    assert!(b
        .alloc_try_with(|| { Result::<(u32, LargeStruct), _>::Err(()) })
        .is_err());
}

#[allow(clippy::large_enum_variant)]
enum LargeEnum {
    Small,
    #[allow(dead_code)]
    Large([u8; 10_000_000]),
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_enum() -> Result<(), ()> {
    let b = Bump::new();

    b.alloc_try_with(|| Ok(LargeEnum::Small))?;

    Ok(())
}

#[test]
#[cfg_attr(debug_assertions, ignore)]
fn alloc_try_with_large_enum_err() {
    let b = Bump::new();

    assert!(b
        .alloc_try_with(|| Result::<LargeEnum, _>::Err(()))
        .is_err());
}
