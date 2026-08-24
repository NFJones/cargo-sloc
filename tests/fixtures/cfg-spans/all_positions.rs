#![cfg_attr(all(), allow(dead_code))]
#![doc = "crate_marker"]

#[cfg(all())]
mod module_marker {}

// standalone_before
#[cfg(all())]
const item_marker: u8 = 1;

struct Fields {
    #[cfg(all())]
    field_marker: u8,
}

enum Variants {
    #[cfg(all())]
    VariantMarker,
}

fn generic_marker<#[cfg(all())] T>() {
    #[cfg(all())]
    let statement_marker = 1;

    #[cfg(all())]
    {
        let expression_marker = statement_marker;
        let _ = expression_marker;
    }

    match statement_marker {
        #[cfg(all())]
        arm_marker @ 1 => {}
        _ => {}
    }

    #[cfg(all())]
    println!("macro_marker");

    let _ = std::marker::PhantomData::<T>;
}

#[cfg_attr(all(), cfg_attr(any(), cfg(any())))]
const nested_marker: u8 = 2;

fn same_line() { let keep_before = 1; #[cfg(all())] const same_line_marker: u8 = 2; let keep_after = 2; }

macro_rules! preserve_tokens {
    ($($tokens:tt)*) => {};
}

preserve_tokens! {
    token_tree_marker
    #[cfg(any())]
    const not_an_attribute_owner: u8 = 0;
}
