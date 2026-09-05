//! Extracting the embedding matrix from a safetensors buffer and widening it to `f32`.
//!
//! The matrix is held as one contiguous `Vec<f32>` of `rows * dim` so a row lookup is a
//! slice at `id * dim`. Widening once at load keeps the per-token path a plain f32 add.

use crate::EMBEDDINGS_TENSOR;
use cyberbrain_core::{Error, Result};
use safetensors::{Dtype, SafeTensors};

pub(crate) struct Matrix {
    pub rows: usize,
    pub dim: usize,
    /// Row-major, `rows * dim` entries, all finite.
    pub data: Vec<f32>,
    /// Storage dtype in the file, for the model card.
    pub dtype: &'static str,
}

pub(crate) fn load_matrix(bytes: &[u8]) -> Result<Matrix> {
    let st = SafeTensors::deserialize(bytes)
        .map_err(|e| Error::Embed(format!("weights are not a valid safetensors file: {e}")))?;

    let names = st.names();
    let name = if names.contains(&EMBEDDINGS_TENSOR) {
        EMBEDDINGS_TENSOR
    } else if names.len() == 1 {
        // A single unnamed-by-convention tensor is unambiguous; accept it.
        names[0]
    } else {
        return Err(Error::Embed(format!(
            "weights hold no tensor named {EMBEDDINGS_TENSOR:?} and are not a single-tensor \
             file; found {:?}",
            names
        )));
    };

    let view = st
        .tensor(name)
        .map_err(|e| Error::Embed(format!("cannot read tensor {name:?}: {e}")))?;

    let shape = view.shape();
    let [rows, dim] = shape else {
        return Err(Error::Embed(format!(
            "tensor {name:?} must be 2-D [vocab, dim], has shape {shape:?}"
        )));
    };
    let (rows, dim) = (*rows, *dim);
    if rows == 0 || dim == 0 {
        return Err(Error::Embed(format!(
            "tensor {name:?} has a zero dimension: shape {shape:?}"
        )));
    }

    let raw = view.data();
    let n = rows * dim;
    let (data, dtype) = match view.dtype() {
        Dtype::F32 => (
            widen(raw, 4, n, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            "f32",
        ),
        Dtype::F16 => (
            widen(raw, 2, n, |b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))),
            "f16",
        ),
        Dtype::BF16 => (
            widen(raw, 2, n, |b| bf16_to_f32(u16::from_le_bytes([b[0], b[1]]))),
            "bf16",
        ),
        // int8 rows are used as-is. A global quantisation scale cancels out under mean
        // pooling followed by L2 normalisation, so cosine ranking is unaffected. A per-row
        // scale would not cancel; a model quantised that way needs a dtype-specific path
        // and this note is where to add it.
        Dtype::I8 => (widen(raw, 1, n, |b| (b[0] as i8) as f32), "int8"),
        other => {
            return Err(Error::Embed(format!(
                "tensor {name:?} has dtype {other:?}; supported: F32, F16, BF16, I8"
            )));
        }
    };

    if data.len() != n {
        return Err(Error::Embed(format!(
            "tensor {name:?} declares {n} elements but carries {}",
            data.len()
        )));
    }

    // A hash match proves the file is the one we were told about, not that its contents
    // are sane. One non-finite weight would spread through every pooled vector it touches.
    if let Some(pos) = data.iter().position(|x| !x.is_finite()) {
        return Err(Error::Embed(format!(
            "tensor {name:?} contains a non-finite value at row {} column {}; refusing to \
             load a matrix that would poison cosine similarity",
            pos / dim,
            pos % dim
        )));
    }

    Ok(Matrix {
        rows,
        dim,
        data,
        dtype,
    })
}

fn widen(raw: &[u8], width: usize, n: usize, f: impl Fn(&[u8]) -> f32) -> Vec<f32> {
    raw.chunks_exact(width).take(n).map(f).collect()
}

/// IEEE 754 binary16 to binary32, bit-exact including subnormals, infinities and NaN.
/// Written here rather than pulling the `half` crate for one function.
pub(crate) fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let frac = (bits & 0x3ff) as u32;
    let out = match exp {
        0 => {
            if frac == 0 {
                sign << 31
            } else {
                // Subnormal: value = frac * 2^-24. Renormalise into an f32 exponent.
                let shift = frac.leading_zeros() - 21; // top set bit becomes the implicit bit 10
                let mant = (frac << shift) & 0x3ff;
                let e = 127 - 15 + 1 - shift;
                (sign << 31) | (e << 23) | (mant << 13)
            }
        }
        31 => (sign << 31) | 0x7f80_0000 | (frac << 13),
        _ => (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13),
    };
    f32::from_bits(out)
}

/// bfloat16 is the top half of a binary32.
pub(crate) fn bf16_to_f32(bits: u16) -> f32 {
    f32::from_bits((bits as u32) << 16)
}
