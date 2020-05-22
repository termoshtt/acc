use proc_macro2::TokenStream;
use quote::quote;
fn get_input_types(func: &syn::ItemFn) -> Vec<syn::Type> {
    func.sig
        .inputs
        .iter()
        .map(|arg| match arg {
            syn::FnArg::Typed(ref val) => &*val.ty,
            _ => panic!("Unsupported kernel input type sigunature"),
        })
        .cloned()
        .collect()
}

fn impl_submodule(ptx_str: &str, func: &syn::ItemFn) -> TokenStream {
    let ident = &func.sig.ident;
    let input_types = get_input_types(func);
    let kernel_name = quote! { #ident }.to_string();
    quote! {
        /// Auto-generated by accel-derive
        mod #ident {
            pub const PTX_STR: &'static str = #ptx_str;

            pub struct Module(::accel::Module);

            impl Module {
                pub fn new(ctx: &::accel::Context) -> ::accel::error::Result<Self> {
                    Ok(Module(::accel::Module::from_str(ctx, PTX_STR)?))
                }
            }

            impl<'arg> ::accel::Launchable<'arg> for Module {
                type Args = (#(&'arg #input_types,)*);
                fn get_kernel(&self) -> ::accel::error::Result<::accel::Kernel> {
                    Ok(self.0.get_kernel(#kernel_name)?)
                }
            }
        }
    }
}

fn caller(func: &syn::ItemFn) -> TokenStream {
    let vis = &func.vis;
    let ident = &func.sig.ident;
    let fn_token = &func.sig.fn_token;
    let input_types = get_input_types(func);
    quote! {
        #vis #fn_token #ident<'arg, G: Into<::accel::Grid>, B: Into<::accel::Block>>(
            ctx: &::accel::Context,
            grid: G,
            block: B,
            args: &(#(&'arg #input_types,)*)
        ) -> ::accel::error::Result<()> {
            use ::accel::Launchable;
            let module = #ident::Module::new(ctx)?;
            module.launch(grid, block, args)?;
            Ok(())
        }
    }
}

pub fn func2caller(ptx_str: &str, func: &syn::ItemFn) -> TokenStream {
    let impl_submodule = impl_submodule(ptx_str, func);
    let caller = caller(func);
    quote! {
        #impl_submodule
        #caller
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    const TEST_KERNEL: &'static str = r#"
    fn kernel_name(arg1: i32, arg2: f64) {}
    "#;

    /// Format TokenStream by rustfmt
    ///
    /// This can test if the input TokenStream is valid in terms of rustfmt.
    fn pretty_print(tt: &impl ToString) -> Result<()> {
        let mut fmt = Command::new("rustfmt")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        fmt.stdin
            .as_mut()
            .unwrap()
            .write(tt.to_string().as_bytes())?;
        let out = fmt.wait_with_output()?;
        println!("{}", String::from_utf8_lossy(&out.stdout));
        Ok(())
    }

    #[test]
    fn impl_submodule() -> Result<()> {
        let func: syn::ItemFn = syn::parse_str(TEST_KERNEL)?;
        let ts = super::impl_submodule("", &func);
        pretty_print(&ts)?;
        Ok(())
    }

    #[test]
    fn caller() -> Result<()> {
        let func: syn::ItemFn = syn::parse_str(TEST_KERNEL)?;
        let ts = super::caller(&func);
        pretty_print(&ts)?;
        Ok(())
    }
}
