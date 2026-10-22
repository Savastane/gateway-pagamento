// tonic::Status is intentionally large; generated RPC signatures trigger
// result_large_err. This crate only re-exports generated bindings.
#![allow(clippy::result_large_err)]

pub mod payment_gateway {
    pub mod v1 {
        tonic::include_proto!("paymentgateway.v1");
    }
}

pub use payment_gateway::v1::*;

pub const FILE_DESCRIPTOR_SET: &[u8] = include_bytes!(concat!(
    env!("OUT_DIR"),
    "/payment_gateway_v1_descriptor.bin"
));
