use proc_macro2::Span;
use syn::{
    braced, bracketed,
    parse::{Parse, ParseBuffer, ParseStream},
    Error, Ident, Result, Token, Type,
};

fn parse_array<F>(input: &ParseStream, mut cb: F) -> Result<()>
where
    F: FnMut(&ParseBuffer) -> Result<()>,
{
    let content;
    let _bracket_token = bracketed!(content in input);

    while !content.is_empty() {
        cb(&content)?;
        if content.is_empty() {
            break;
        }
        let _punct: Token![,] = content.parse()?;
    }

    Ok(())
}

fn parse_fields<F>(input: &ParseStream, mut cb: F) -> Result<()>
where
    F: FnMut(&ParseBuffer) -> Result<()>,
{
    let content;
    let _brace_token = braced!(content in input);

    while !content.is_empty() {
        cb(&content)?;
        if content.is_empty() {
            break;
        }
        let _punct: Token![,] = content.parse()?;
    }

    Ok(())
}

pub struct Global {
    pub interface: Type,
    pub data: Type,
}

impl Parse for Global {
    fn parse(input: ParseStream) -> Result<Self> {
        let ident: Ident = input.parse()?;
        if ident != "Global" {
            return Err(Error::new(ident.span(), "expected `Bundle`"));
        }

        let (interface, data) = parse_interface_data(&input, ident.span())?;
        Ok(Self { interface, data })
    }
}

pub struct Resource {
    pub interface: Type,
    pub data: Type,
}

impl Parse for Resource {
    fn parse(input: ParseStream) -> Result<Self> {
        let ident: Ident = input.parse()?;
        if ident != "Resource" {
            return Err(Error::new(ident.span(), "expected `Bundle`"));
        }

        let (interface, data) = parse_interface_data(&input, ident.span())?;
        Ok(Self { interface, data })
    }
}

fn parse_interface_data(input: &ParseStream, span: Span) -> Result<(Type, Type)> {
    let mut interface = None;
    let mut data = None;

    parse_fields(input, |input| {
        let member: Ident = input.parse()?;
        let _colon_token: Token![:] = input.parse()?;

        if member == "interface" {
            interface = Some(input.parse()?);
        } else if member == "data" {
            data = Some(input.parse()?);
        } else {
            return Err(Error::new(
                member.span(),
                "Unexpected field, expected `interface, data`",
            ));
        }

        Ok(())
    })?;

    let interface = interface.ok_or(Error::new(span, "Field `interface` not found"))?;
    let data = data.ok_or(Error::new(span, "Field `data` not found"))?;

    Ok((interface, data))
}

pub struct Bundle {
    pub dispatch_to: Type,
    pub globals: Vec<Global>,
    pub resources: Vec<Resource>,
}

impl Parse for Bundle {
    fn parse(input: ParseStream) -> Result<Self> {
        let ident: Ident = input.parse()?;

        if ident != "Bundle" {
            return Err(Error::new(ident.span(), "expected `Bundle`"));
        }

        let mut dispatch_to = None;
        let mut globals = None;
        let mut resources = None;

        parse_fields(&input, |input| {
            let member: Ident = input.parse()?;
            let _colon_token: Token![:] = input.parse()?;

            if member == "dispatch_to" {
                dispatch_to = Some(input.parse()?);
            } else if member == "globals" {
                let mut elements = Vec::new();
                parse_array(&input, |input| {
                    elements.push(input.parse()?);
                    Ok(())
                })?;
                globals = Some(elements);
            } else if member == "resources" {
                let mut elements = Vec::new();
                parse_array(&input, |input| {
                    elements.push(input.parse()?);
                    Ok(())
                })?;
                resources = Some(elements);
            } else {
                return Err(Error::new(
                    member.span(),
                    "Unexpected field, expected `dispatch_to, globals, resources`",
                ));
            }

            Ok(())
        })?;

        let dispatch_to = dispatch_to.ok_or(Error::new(ident.span(), "Field `dispatch_to` not found"))?;
        let globals = globals.ok_or(Error::new(ident.span(), "Field `globals` not found"))?;
        let resources = resources.ok_or(Error::new(ident.span(), "Field `resources` not found"))?;

        Ok(Self {
            dispatch_to,
            globals,
            resources,
        })
    }
}
