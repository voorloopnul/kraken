//! Everything to do with the Pi coding agent: the RPC process, its on-disk
//! configuration, its session files, its model catalogue, and the controller
//! that turns a stream of agent events into a transcript.

pub mod catalogue;
pub mod config;
pub mod controller;
pub mod rpc;
pub mod sessions;
