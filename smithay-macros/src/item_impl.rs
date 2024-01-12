use syn::{
    braced,
    parse::{Parse, ParseStream},
    Generics, Ident, Lifetime, Result, Token, Type,
};

/// Parses: `impl<T> State<T> {}` item
///
/// `impl` is optional
/// `{}` is optional
pub struct ItemImpl {
    pub self_ty: Type,
    pub generics: Generics,
}

impl Parse for ItemImpl {
    fn parse(input: ParseStream) -> Result<Self> {
        let has_impl = input.peek(Token![impl]);

        if has_impl {
            let _impl_token: Token![impl] = input.parse()?;
        }

        let has_generics = input.peek(Token![<])
            && (input.peek2(Token![>])
                || input.peek2(Token![#])
                || (input.peek2(Ident) || input.peek2(Lifetime))
                    && (input.peek3(Token![:])
                        || input.peek3(Token![,])
                        || input.peek3(Token![>])
                        || input.peek3(Token![=]))
                || input.peek2(Token![const]));

        let mut generics: Generics = if has_impl && has_generics {
            input.parse()?
        } else {
            Generics::default()
        };

        let self_ty: Type = input.parse()?;

        generics.where_clause = input.parse()?;

        if input.peek(syn::token::Brace) {
            let _content;
            let _brace_token = braced!(_content in input);
        }

        Ok(ItemImpl { self_ty, generics })
    }
}
