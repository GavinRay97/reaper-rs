/// Executed whenever Cargo builds reaper-rs
fn main() {
    #[cfg(feature = "generate-low-level-reaper")]
    codegen::generate_all();
    compile_glue_code();
}

/// Compiles C++ glue code. This is necessary to interact with those parts of the REAPER SDK that
/// use pure virtual interface classes and therefore the C++ ABI.
fn compile_glue_code() {
    cc::Build::new()
        .cpp(true)
        .file("src/low_level/control_surface.cpp")
        .file("src/low_level/midi.cpp")
        .compile("glue");
}

#[cfg(feature = "generate-low-level-reaper")]
mod codegen {
    /// Generates both low-level `bindings.rs` and `reaper.rs`
    pub fn generate_all() {
        generate_bindings();
        generate_reaper();
    }

    /// Generates the low-level `bindings.rs` file from REAPER C++ headers
    fn generate_bindings() {
        println!("cargo:rerun-if-changed=src/low_level/bindgen.hpp");
        let bindings = bindgen::Builder::default()
            .header("src/low_level/bindgen.hpp")
            .opaque_type("timex")
            .derive_eq(true)
            .derive_partialeq(true)
            .derive_hash(true)
            .clang_arg("-xc++")
            .enable_cxx_namespaces()
            .raw_line("#![allow(non_upper_case_globals)]")
            .raw_line("#![allow(non_camel_case_types)]")
            .raw_line("#![allow(non_snake_case)]")
            .raw_line("#![allow(dead_code)]")
            .whitelist_var("reaper_functions::.*")
            .whitelist_var("CSURF_EXT_.*")
            .whitelist_var("REAPER_PLUGIN_VERSION")
            .whitelist_var("UNDO_STATE_.*")
            .whitelist_type("HINSTANCE")
            .whitelist_type("reaper_plugin_info_t")
            .whitelist_type("gaccel_register_t")
            .whitelist_type("audio_hook_register_t")
            .whitelist_type("KbdSectionInfo")
            .whitelist_type("GUID")
            .whitelist_function("GetActiveWindow")
            .whitelist_function("reaper_rs_control_surface::.*")
            .whitelist_function("reaper_rs_midi::.*")
            // Tell cargo to invalidate the built crate whenever any of the
            // included header files changed.
            .parse_callbacks(Box::new(bindgen::CargoCallbacks))
            .generate()
            .expect("Unable to generate bindings");
        let out_path = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        bindings
            .write_to_file(out_path.join("src/low_level/bindings.rs"))
            .expect("Couldn't write bindings!");
    }

    /// Generates the low-level `reaper.rs` file from the previously generated `bindings.rs`
    fn generate_reaper() {
        use quote::ToTokens;
        use std::path::Path;
        use syn::{
            AngleBracketedGenericArguments, ForeignItem, ForeignItemStatic, GenericArgument, Ident,
            Item, ItemForeignMod, ItemMod, Pat, PatIdent, PathArguments, PathSegment, Type,
            TypeBareFn,
        };

        generate();

        fn experiment(ptr: ReaperFnPtr) {
            use proc_macro2::Span;
            use syn::punctuated::Punctuated;
            use syn::token::{And, Brace, Colon, Colon2, Comma, Fn, Paren, Pub, SelfValue, Unsafe};
            use syn::{
                Block, Expr, ExprCall, ExprPath, FnArg, ImplItem, ImplItemMethod, PatType, Path,
                PathSegment, Receiver, ReturnType, Signature, VisPublic, Visibility,
            };
            let ReaperFnPtr { ident, fn_type } = ptr;
            let actual_call = Expr::Call(ExprCall {
                attrs: vec![],
                func: Box::new(Expr::Path(ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: Path {
                        leading_colon: None,
                        segments: {
                            let mut p = Punctuated::new();
                            let ps = PathSegment {
                                ident: Ident::new("f", Span::call_site()),
                                arguments: Default::default(),
                            };
                            p.push(ps);
                            p
                        },
                    },
                })),
                paren_token: Paren {
                    span: Span::call_site(),
                },
                args: fn_type
                    .inputs
                    .iter()
                    .map(|a| {
                        Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: Path {
                                leading_colon: None,
                                segments: {
                                    let mut p = Punctuated::new();
                                    let ps = PathSegment {
                                        ident: a.name.clone().unwrap().0,
                                        arguments: Default::default(),
                                    };
                                    p.push(ps);
                                    p
                                },
                            },
                        })
                    })
                    .collect(),
            });
            let tree = ImplItem::Method(ImplItemMethod {
                attrs: vec![],
                vis: Visibility::Public(VisPublic {
                    pub_token: Pub {
                        span: Span::call_site(),
                    },
                }),
                defaultness: None,
                sig: Signature {
                    constness: None,
                    asyncness: None,
                    unsafety: Some(Unsafe {
                        span: Span::call_site(),
                    }),
                    abi: None,
                    fn_token: Fn {
                        span: Span::call_site(),
                    },
                    ident: ident.clone(),
                    generics: Default::default(),
                    paren_token: Paren {
                        span: Span::call_site(),
                    },
                    inputs: {
                        let receiver = FnArg::Receiver(Receiver {
                            attrs: vec![],
                            reference: Some((
                                And {
                                    spans: [Span::call_site()],
                                },
                                None,
                            )),
                            mutability: None,
                            self_token: SelfValue {
                                span: Span::call_site(),
                            },
                        });
                        let actual_args = fn_type.inputs.iter().map(|a| {
                            FnArg::Typed(PatType {
                                attrs: vec![],
                                pat: Box::new(Pat::Ident(PatIdent {
                                    attrs: vec![],
                                    by_ref: None,
                                    mutability: None,
                                    ident: a.name.clone().unwrap().0,
                                    subpat: None,
                                })),
                                colon_token: Colon {
                                    spans: [Span::call_site()],
                                },
                                ty: Box::new(a.ty.clone()),
                            })
                        });
                        std::iter::once(receiver).chain(actual_args).collect()
                    },
                    variadic: None,
                    output: fn_type.output,
                },
                block: syn::parse_quote! {
                    {
                        match self.pointers.#ident {
                            None => panic!(format!(
                                "Attempt to use a REAPER function that has not been loaded: {}",
                                stringify!(#ident)
                            )),
                            Some(f) => #actual_call,
                        }
                    }
                },
            });
            let result = quote::quote! {
                #tree
            };
            std::fs::write("src/low_level/experiment.rs", result.to_string())
                .expect("Unable to write file");
        }

        fn generate() {
            use std::env;
            use std::fs::File;
            use std::io::Read;
            use std::process;
            let mut file =
                File::open("src/low_level/bindings.rs").expect("Unable to open bindings.rs");
            let mut src = String::new();
            file.read_to_string(&mut src).expect("Unable to read file");
            let file = syn::parse_file(&src).expect("Unable to parse file");
            let fn_ptrs: Vec<_> = filter_reaper_fn_ptr_items(&file)
                .into_iter()
                .map(map_to_reaper_fn_ptr)
                .collect();
            experiment(fn_ptrs.get(1).unwrap().clone());
            let idents: Vec<_> = fn_ptrs.iter().map(|p| p.ident.clone()).collect();
            let fn_types: Vec<TypeBareFn> = fn_ptrs.iter().map(|p| p.fn_type.clone()).collect();
            let result = quote::quote! {
                /* automatically generated by build script */
                #![allow(non_upper_case_globals)]
                #![allow(non_camel_case_types)]
                #![allow(non_snake_case)]

                use super::{bindings::root, ReaperPluginContext};
                use c_str_macro::c_str;

                /// This is the low-level API access point to all REAPER functions. In order to use it, you first
                /// must obtain an instance of this struct by invoking [`load`](struct.Reaper.html#method.load).
                ///
                /// Please note that it's possible that functions are *not available*. This can be the case if
                /// the user runs your plug-in in an older version of REAPER which doesn't have that function yet.
                /// Therefore each function in this struct is actually a function pointer wrapped
                /// in an `Option`. If you are sure your function will be there, you can just unwrap the option.
                /// The medium-level API doesn't have this distinction anymore. It just unwraps the options
                /// automatically for the sake of convenience.
                #[derive(Default)]
                pub struct Reaper {
                    #(
                        pub #idents: Option<#fn_types>,
                    )*
                }

                impl Reaper {
                    /// Loads all available REAPER functions plug-in context and returns a `Reaper` instance
                    /// which allows you to call these functions.
                    pub fn load(context: &ReaperPluginContext) -> Reaper {
                        let get_func = &context.function_provider;
                        unsafe {
                            Reaper {
                                #(
                                    #idents: std::mem::transmute(get_func(c_str!(stringify!(#idents)))),
                                )*
                            }
                        }
                    }
                }
            };
            std::fs::write("src/low_level/reaper.rs", result.to_string())
                .expect("Unable to write file");
        }

        fn filter_reaper_fn_ptr_items(file: &syn::File) -> Vec<&ForeignItemStatic> {
            let (_, root_mod_items) = match file.items.as_slice() {
                [Item::Mod(ItemMod {
                    ident: id,
                    content: Some(c),
                    ..
                })] if id == "root" => c,
                _ => panic!("root mod not found"),
            };
            let (_, reaper_functions_mod_items) = root_mod_items
                .iter()
                .find_map(|item| match item {
                    Item::Mod(ItemMod {
                        ident: id,
                        content: Some(c),
                        ..
                    }) if id == "reaper_functions" => Some(c),
                    _ => None,
                })
                .expect("reaper_functions mod not found");
            reaper_functions_mod_items
                .iter()
                .filter_map(|item| match item {
                    Item::ForeignMod(ItemForeignMod { items, .. }) => match items.as_slice() {
                        [ForeignItem::Static(i)] => Some(i),
                        _ => None,
                    },
                    _ => None,
                })
                .collect()
        }

        fn map_to_reaper_fn_ptr(item: &ForeignItemStatic) -> ReaperFnPtr {
            let option_segment = match &*item.ty {
                Type::Path(p) => p
                    .path
                    .segments
                    .iter()
                    .find(|seg| seg.ident == "Option")
                    .expect("Option not found in fn ptr item"),
                _ => panic!("fn ptr item doesn't have path type"),
            };
            let bare_fn = match &option_segment.arguments {
                PathArguments::AngleBracketed(a) => {
                    let generic_arg = a.args.first().expect("Angle bracket must have arg");
                    match generic_arg {
                        GenericArgument::Type(Type::BareFn(bare_fn)) => bare_fn,
                        _ => panic!("Generic argument is not a BareFn"),
                    }
                }
                _ => panic!("Option type doesn't have angle bracket"),
            };
            ReaperFnPtr {
                ident: item.ident.clone(),
                fn_type: bare_fn.clone(),
            }
        }

        #[derive(Clone)]
        struct ReaperFnPtr {
            ident: Ident,
            fn_type: TypeBareFn,
        }
    }
}
