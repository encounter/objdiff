use std::fmt::{LowerHex, UpperHex};

use num_traits::PrimInt;

// https://stackoverflow.com/questions/44711012/how-do-i-format-a-signed-integer-to-a-sign-aware-hexadecimal-representation
pub(crate) struct ReallySigned<N: PrimInt>(pub(crate) N);

impl<N: PrimInt> LowerHex for ReallySigned<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let num = self.0.to_i32().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:x}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}

impl<N: PrimInt> UpperHex for ReallySigned<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let num = self.0.to_i32().unwrap();
        let prefix = if f.alternate() { "0x" } else { "" };
        let bare_hex = format!("{:X}", num.abs());
        f.pad_integral(num >= 0, prefix, &bare_hex)
    }
}
