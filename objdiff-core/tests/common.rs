use objdiff_core::{
    diff::{DiffObjConfig, SymbolDiff},
    obj::Object,
};

pub fn display_diff(
    obj: &Object,
    diff: &SymbolDiff,
    symbol_idx: usize,
    diff_config: &DiffObjConfig,
) -> String {
    let mut output = String::new();
    for row in &diff.instruction_rows {
        output.push('[');
        let mut separator = false;
        objdiff_core::diff::display::display_row(
            &obj,
            symbol_idx,
            row,
            &diff_config,
            |text, diff_idx| {
                if separator {
                    output.push_str(", ");
                } else {
                    separator = true;
                }
                output.push_str(&format!("({:?}, {:?})", text, diff_idx.get()));
                Ok(())
            },
        )
        .unwrap();
        output.push_str("]\n");
    }
    output
}

#[repr(C)]
pub struct AlignedAs<Align, Bytes: ?Sized> {
    pub _align: [Align; 0],
    pub bytes: Bytes,
}

#[macro_export]
macro_rules! include_bytes_align_as {
    ($align_ty:ty, $path:literal) => {{
        static ALIGNED: &common::AlignedAs<$align_ty, [u8]> =
            &common::AlignedAs { _align: [], bytes: *include_bytes!($path) };
        &ALIGNED.bytes
    }};
}

#[macro_export]
macro_rules! include_object {
    ($path:literal) => {
        include_bytes_align_as!(u32, $path)
    };
}
