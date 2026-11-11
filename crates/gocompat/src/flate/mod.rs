//! Go `compress/flate`: a byte-exact port of the compressor (all levels, including
//! `HuffmanOnly`, `NoCompression` and the `BestSpeed` fast encoder) and of the decompressor.

mod deflate;
mod huffman;
mod inflate;

pub use deflate::{
    BEST_COMPRESSION, BEST_SPEED, DEFAULT_COMPRESSION, HUFFMAN_ONLY, InvalidLevelError, NO_COMPRESSION,
    Writer, compress,
};
pub use inflate::{InflateError, decompress};
pub(crate) use inflate::Inflater;
