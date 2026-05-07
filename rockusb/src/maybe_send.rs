#[cfg(not(target_family = "wasm"))]
mod platform {
    pub use std::marker::Send as MaybeSend;

    #[cfg(feature = "async")]
    pub type BoxFuture<'a, T> = futures::future::BoxFuture<'a, T>;
}

#[cfg(target_family = "wasm")]
mod platform {
    pub trait MaybeSend {}
    impl<T> MaybeSend for T {}

    #[cfg(feature = "async")]
    pub type BoxFuture<'a, T> = futures::future::LocalBoxFuture<'a, T>;
}

pub use platform::*;
