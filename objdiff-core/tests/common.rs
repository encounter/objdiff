use objdiff_core::{
    diff::{DiffObjConfig, SymbolDiff, display::DiffTextSegment},
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
        objdiff_core::diff::display::display_row(&obj, symbol_idx, row, &diff_config, |segment| {
            if separator {
                output.push_str(", ");
            } else {
                separator = true;
            }
            let DiffTextSegment { text, color, pad_to } = segment;
            output.push_str(&format!("({:?}, {:?}, {:?})", text, color, pad_to));
            Ok(())
        })
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
