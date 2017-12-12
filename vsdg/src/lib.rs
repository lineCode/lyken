#![feature(conservative_impl_trait, nonzero)]
#![feature(slice_patterns)]

extern crate core;
extern crate cretonne_codegen;

pub mod graph;
pub mod construct {
    pub mod cretonne;
}
