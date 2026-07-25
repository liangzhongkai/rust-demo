//! 过程宏 demo：与 `crates/macros` 里的声明宏形成对照。
//!
//! - `#[derive(EventJson)]` — derive 宏，读 struct 字段元信息生成 `to_json`
//! - `sol_dispatch!`         — 函数式宏，编译期 keccak256 算 selector
//! - `#[cold_handler]`       — 属性宏，给函数打上 cold / inline(never)

mod event_json;
mod sol_dispatch;

use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemFn};

#[proc_macro_derive(EventJson)]
pub fn derive_event_json(input: TokenStream) -> TokenStream {
    event_json::expand(input)
}

#[proc_macro]
pub fn sol_dispatch(input: TokenStream) -> TokenStream {
    sol_dispatch::expand(input)
}

/// 把函数标记为冷路径，等价于同时加 `#[cold]` 和 `#[inline(never)]`。
#[proc_macro_attribute]
pub fn cold_handler(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(item as ItemFn);
    func.attrs
        .push(syn::parse_quote!(#[cold]));
    func.attrs
        .push(syn::parse_quote!(#[inline(never)]));
    quote::quote!(#func).into()
}
