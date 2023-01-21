/// Adapted from https://crates.io/crates/rapidfuzz
// Copyright 2020 maxbachmann
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum LevEditType {
    Replace,
    Insert,
    Delete,
}

#[derive(Debug, PartialEq, Eq)]
pub struct LevEditOp {
    pub op_type: LevEditType, /* editing operation type */
    pub first_start: usize,   /* source block position */
    pub second_start: usize,  /* destination position */
}

pub fn editops_find<T>(query: &[T], choice: &[T]) -> Vec<LevEditOp>
where T: PartialEq {
    let Affix { prefix_len, suffix_len } = Affix::find(query, choice);

    let first_string = &query[prefix_len..query.len() - suffix_len];
    let second_string = &choice[prefix_len..choice.len() - suffix_len];

    let matrix_columns = first_string.len() + 1;
    let matrix_rows = second_string.len() + 1;

    // TODO maybe use an actual matrix for readability
    let mut cache_matrix: Vec<usize> = vec![0; matrix_rows * matrix_columns];
    for (i, elem) in cache_matrix.iter_mut().enumerate().take(matrix_rows) {
        *elem = i;
    }
    for i in 1..matrix_columns {
        cache_matrix[matrix_rows * i] = i;
    }

    for (i, char1) in first_string.iter().enumerate() {
        let mut prev = i * matrix_rows;
        let current = prev + matrix_rows;
        let mut x = i + 1;
        for (p, char2p) in second_string.iter().enumerate() {
            let mut c3 = cache_matrix[prev] + (char1 != char2p) as usize;
            prev += 1;
            x += 1;
            if x >= c3 {
                x = c3;
            }
            c3 = cache_matrix[prev] + 1;
            if x > c3 {
                x = c3;
            }
            cache_matrix[current + 1 + p] = x;
        }
    }
    editops_from_cost_matrix::<T>(matrix_columns, matrix_rows, prefix_len, cache_matrix)
}

fn editops_from_cost_matrix<T>(
    len1: usize,
    len2: usize,
    prefix_len: usize,
    cache_matrix: Vec<usize>,
) -> Vec<LevEditOp>
where
    T: PartialEq,
{
    let mut ops = Vec::with_capacity(cache_matrix[len1 * len2 - 1]);
    let mut dir = 0;
    let mut i = len1 - 1;
    let mut j = len2 - 1;
    let mut p = len1 * len2 - 1;

    //TODO this is still pretty ugly
    while i > 0 || j > 0 {
        let current_value = cache_matrix[p];

        // More than one operation can be possible at a time. We use `dir` to
        // decide when ambiguous.
        let is_insert = j > 0 && current_value == cache_matrix[p - 1] + 1;
        let is_delete = i > 0 && current_value == cache_matrix[p - len2] + 1;
        let is_replace = i > 0 && j > 0 && current_value == cache_matrix[p - len2 - 1] + 1;

        let (op_type, new_dir) = match (dir, is_insert, is_delete, is_replace) {
            (_, false, false, false) => (None, 0),
            (-1, true, _, _) => (Some(LevEditType::Insert), -1),
            (1, _, true, _) => (Some(LevEditType::Delete), 1),
            (_, _, _, true) => (Some(LevEditType::Replace), 0),
            (0, true, _, _) => (Some(LevEditType::Insert), -1),
            (0, _, true, _) => (Some(LevEditType::Delete), 1),
            _ => panic!("something went terribly wrong"),
        };

        match new_dir {
            -1 => {
                j -= 1;
                p -= 1;
            }
            1 => {
                i -= 1;
                p -= len2;
            }
            0 => {
                i -= 1;
                j -= 1;
                p -= len2 + 1;
            }
            _ => panic!("something went terribly wrong"),
        };
        dir = new_dir;

        if let Some(op_type) = op_type {
            ops.insert(0, LevEditOp {
                op_type,
                first_start: i + prefix_len,
                second_start: j + prefix_len,
            });
        }
    }

    ops
}

pub struct Affix {
    pub prefix_len: usize,
    pub suffix_len: usize,
}

impl Affix {
    pub fn find<T>(s1: &[T], s2: &[T]) -> Affix
    where T: PartialEq {
        let prefix_len = s1.iter().zip(s2.iter()).take_while(|t| t.0 == t.1).count();
        let suffix_len = s1[prefix_len..]
            .iter()
            .rev()
            .zip(s2[prefix_len..].iter().rev())
            .take_while(|t| t.0 == t.1)
            .count();

        Affix { prefix_len, suffix_len }
    }
}
