use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

pub fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    name,
                    "EventJson requires a struct with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(name, "EventJson only supports structs")
                .to_compile_error()
                .into();
        }
    };

    let field_idents = fields.iter().map(|f| f.ident.as_ref().unwrap());
    let field_names = field_idents.clone().map(syn::Ident::to_string);

    quote! {
        impl #name {
            /// 序列化为 JSON；字段名与 struct 定义一致，顺序固定。
            pub fn to_json(&self) -> String {
                let mut s = String::from("{");
                #(
                    s.push_str(&format!(
                        "\"{field_name}\":\"{:?}\",",
                        self.#field_idents,
                        field_name = #field_names,
                    ));
                )*
                if s.len() > 1 {
                    s.pop();
                }
                s.push('}');
                s
            }
        }
    }
    .into()
}
