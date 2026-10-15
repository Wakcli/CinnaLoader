//! Native plumbing for the loader: find the client's functions by name in its own
//! symbol table, turn those into live addresses, and hook them.
//!
//! Cinnabar's packaged binary is not stripped, so the loader can resolve a game
//! function from the `.symtab` of `/proc/self/exe` at runtime instead of hardcoding
//! offsets. See `native/README` in the notes for the rationale.

pub mod detour;
pub mod symbols;
