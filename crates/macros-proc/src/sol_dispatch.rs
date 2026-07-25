use proc_macro::TokenStream;
use proc_macro2::Ident;
use quote::quote;
use sha3::{Digest, Keccak256};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
    Token,
};

struct DispatchEntry {
    signature: String,
    handler: Ident,
}

impl Parse for DispatchEntry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let func: Ident = input.parse()?;
        let content;
        syn::parenthesized!(content in input);

        let mut args = Vec::new();
        while !content.is_empty() {
            let ty: Ident = content.parse()?;
            args.push(ty.to_string());
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        input.parse::<Token![=>]>()?;
        let handler: Ident = input.parse()?;

        Ok(Self {
            signature: format!("{}({})", func, args.join(",")),
            handler,
        })
    }
}

struct DispatchInput {
    entries: Vec<DispatchEntry>,
}

impl Parse for DispatchInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            entries.push(input.parse()?);
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(Self { entries })
    }
}

fn evm_selector(signature: &str) -> u32 {
    let hash = Keccak256::digest(signature.as_bytes());
    u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]])
}

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DispatchInput);

    if input.entries.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "sol_dispatch! requires at least one entry",
        )
        .to_compile_error()
        .into();
    }

    let selectors = input.entries.iter().map(|entry| {
        let sel = evm_selector(&entry.signature);
        let handler = &entry.handler;
        let sig = &entry.signature;
        quote! { (#sel, #sig, stringify!(#handler)) }
    });

    let match_arms = input.entries.iter().map(|entry| {
        let sel = evm_selector(&entry.signature);
        let handler = &entry.handler;
        quote! { #sel => #handler(calldata), }
    });

    quote! {
        /// selector 常量表：`(u32, signature, handler_name)`，便于 fuzz / 调试。
        pub const SELECTORS: &[(u32, &str, &str)] = &[
            #( #selectors, )*
        ];

        pub fn dispatch(selector: u32, calldata: &[u8]) -> Result<Vec<u8>, VmError> {
            match selector {
                #( #match_arms )*
                _ => Err(VmError::Unauthorized),
            }
        }
    }
    .into()
}
