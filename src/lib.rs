pub mod parser;
pub mod node;
pub mod typing;
pub mod id;
pub mod codegen;

#[macro_use]
extern crate nom;

#[link(name = "ffi")]
extern "C" {}

#[macro_use]
extern crate lazy_static;
