//! Extracting the embedding matrix from safetensors and presenting it as `f32` rows.
//!
//! The matrix is one contiguous row-major `[f32]` of `rows * dim`, so a row lookup is a
//! slice at `id * dim`. An `f32` little-endian file is read straight from a memory mapping
//! (SPEC §6.5: "weights are memory-mapped rather than copied"); only the rows a text
//! touches are ever paged in, and the page cache is shared between processes. Every other
//! dtype is widened once into an owned buffer.
//!
//! Until 2026-09-25 every file was read whole and then widened, f32 included: 512 MB read
//! plus a second 512 MB copy, 0.52 s and 1 GB of every CLI `recall`.

use crate::EMBEDDINGS_TENSOR;
use cyberbrain_core::{Error, Result};
use memmap2::Mmap;
use safetensors::{Dtype, SafeTensors};
use std::ops::Deref;

pub(crate) struct Matrix {
    pub rows: usize,
    pub dim: usize,
    /// Row-major, `rows * dim` entries, all finite.
    pub data: Floats,
    /// Storage dtype in the file, for the model card.
    pub dtype: &'static str,
}

/// The matrix's numbers, either owned (widened) or borrowed from the file's mapping.
pub(crate) enum Floats {
    Owned(Vec<f32>),
    Mapped { map: Mmap, start: usize, len: usize },
}

impl Deref for Floats {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        match self {
            Floats::Owned(v) => v,
            // SAFETY: `Mapped` is only built by `mapped_f32`, which checked that
            // `start..start + len * 4` lies inside the mapping, that the address is aligned
            // for f32 and that the target is little-endian. Any bit pattern is a valid f32.
            // The mapping lives as long as `self`.
            Floats::Mapped { map, start, len } => unsafe {
                std::slice::from_raw_parts(map.as_ptr().add(*start).cast::<f32>(), *len)
            },
        }
    }
}

/// The shape of the embeddings tensor, read from the header alone.
pub(crate) fn tensor_shape(bytes: &[u8]) -> Result<(usize, usize)> {
    let st = deserialize(bytes)?;
    let (name, view) = embeddings(&st)?;
    shape_of(name, view.shape())
}

fn deserialize(bytes: &[u8]) -> Result<SafeTensors<'_>> {
    SafeTensors::deserialize(bytes)
        .map_err(|e| Error::Embed(format!("weights are not a valid safetensors file: {e}")))
}

fn shape_of(name: &str, shape: &[usize]) -> Result<(usize, usize)> {
    let [rows, dim] = shape else {
        return Err(Error::Embed(format!(
            "tensor {name:?} must be 2-D [vocab, dim], has shape {shape:?}"
        )));
    };
    if *rows == 0 || *dim == 0 {
        return Err(Error::Embed(format!(
            "tensor {name:?} has a zero dimension: shape {shape:?}"
        )));
    }
    Ok((*rows, *dim))
}

fn embeddings<'a>(
    st: &'a SafeTensors<'a>,
) -> Result<(&'a str, safetensors::tensor::TensorView<'a>)> {
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
    Ok((name, view))
}

/// The matrix from a mapped file. `check_finite` is false only when this very file (same
/// size, mtime and inode) passed the full load before (`artefact::VerifiedCache`): the scan
/// touches every page, which is exactly what mapping is meant to avoid.
pub(crate) fn load_matrix_mapped(map: Mmap, check_finite: bool) -> Result<Matrix> {
    let (rows, dim, range) = {
        let st = deserialize(&map)?;
        let (name, view) = embeddings(&st)?;
        let (rows, dim) = shape_of(name, view.shape())?;
        let raw = view.data();
        let start = raw.as_ptr() as usize - map.as_ptr() as usize;
        let fits = view.dtype() == Dtype::F32
            && cfg!(target_endian = "little")
            && raw.len() == rows * dim * 4
            && (raw.as_ptr() as usize).is_multiple_of(std::mem::align_of::<f32>());
        (rows, dim, fits.then_some(start))
    };
    let Some(start) = range else {
        // Another dtype or an unaligned tensor: widen as before.
        return load_matrix_checked(&map, check_finite);
    };
    let matrix = Matrix {
        rows,
        dim,
        data: Floats::Mapped {
            map,
            start,
            len: rows * dim,
        },
        dtype: "f32",
    };
    if check_finite {
        check_all_finite(EMBEDDINGS_TENSOR, &matrix.data, dim)?;
    }
    Ok(matrix)
}

fn load_matrix_checked(bytes: &[u8], check_finite: bool) -> Result<Matrix> {
    let st = deserialize(bytes)?;
    let (name, view) = embeddings(&st)?;
    let (rows, dim) = shape_of(name, view.shape())?;

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

    if check_finite {
        check_all_finite(name, &data, dim)?;
    }

    Ok(Matrix {
        rows,
        dim,
        data: Floats::Owned(data),
        dtype,
    })
}

/// A hash match proves the file is the one we were told about, not that its contents are
/// sane. One non-finite weight would spread through every pooled vector it touches.
fn check_all_finite(name: &str, data: &[f32], dim: usize) -> Result<()> {
    if let Some(pos) = data.iter().position(|x| !x.is_finite()) {
        return Err(Error::Embed(format!(
            "tensor {name:?} contains a non-finite value at row {} column {}; refusing to \
             load a matrix that would poison cosine similarity",
            pos / dim,
            pos % dim
        )));
    }
    Ok(())
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
