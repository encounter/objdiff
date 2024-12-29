use std::{
    fs::File,
    path::{Path, PathBuf},
};

use heck::{ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};

#[derive(Debug, serde::Deserialize)]
pub struct ConfigSchema {
    pub properties: Vec<ConfigProperty>,
    pub groups: Vec<ConfigGroup>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ConfigProperty {
    #[serde(rename = "boolean")]
    Boolean(ConfigPropertyBoolean),
    #[serde(rename = "choice")]
    Choice(ConfigPropertyChoice),
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigPropertyBase {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigPropertyBoolean {
    #[serde(flatten)]
    pub base: ConfigPropertyBase,
    pub default: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigPropertyChoice {
    #[serde(flatten)]
    pub base: ConfigPropertyBase,
    pub default: String,
    pub items: Vec<ConfigPropertyChoiceItem>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigPropertyChoiceItem {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfigGroup {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub properties: Vec<String>,
}

fn build_doc(name: &str, description: Option<&str>) -> TokenStream {
    let mut doc = format!(" {}", name);
    let mut out = quote! { #[doc = #doc] };
    if let Some(description) = description {
        doc = format!(" {}", description);
        out.extend(quote! { #[doc = ""] });
        out.extend(quote! { #[doc = #doc] });
    }
    out
}

pub fn generate_diff_config() {
    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config-schema.json");
    println!("cargo:rerun-if-changed={}", schema_path.display());
    let schema_file = File::open(schema_path).expect("Failed to open config schema file");
    let schema: ConfigSchema =
        serde_json::from_reader(schema_file).expect("Failed to parse config schema");

    let mut enums = TokenStream::new();
    for property in &schema.properties {
        let ConfigProperty::Choice(choice) = property else {
            continue;
        };
        let enum_ident = format_ident!("{}", choice.base.id.to_upper_camel_case());
        let mut variants = TokenStream::new();
        let mut full_variants = TokenStream::new();
        let mut variant_info = TokenStream::new();
        let mut variant_to_str = TokenStream::new();
        let mut variant_to_name = TokenStream::new();
        let mut variant_to_description = TokenStream::new();
        let mut variant_from_str = TokenStream::new();
        for item in &choice.items {
            let variant_name = item.value.to_upper_camel_case();
            let variant_ident = format_ident!("{}", variant_name);
            let is_default = item.value == choice.default;
            variants.extend(build_doc(&item.name, item.description.as_deref()));
            if is_default {
                variants.extend(quote! { #[default] });
            }
            let value = &item.value;
            variants.extend(quote! {
                #[serde(rename = #value, alias = #variant_name)]
                #variant_ident,
            });
            full_variants.extend(quote! { #enum_ident::#variant_ident, });
            variant_to_str.extend(quote! { #enum_ident::#variant_ident => #value, });
            let name = &item.name;
            variant_to_name.extend(quote! { #enum_ident::#variant_ident => #name, });
            if let Some(description) = &item.description {
                variant_to_description.extend(quote! {
                    #enum_ident::#variant_ident => Some(#description),
                });
            } else {
                variant_to_description.extend(quote! {
                    #enum_ident::#variant_ident => None,
                });
            }
            let description = if let Some(description) = &item.description {
                quote! { Some(#description) }
            } else {
                quote! { None }
            };
            variant_info.extend(quote! {
                ConfigEnumVariantInfo {
                    value: #value,
                    name: #name,
                    description: #description,
                    is_default: #is_default,
                },
            });
            variant_from_str.extend(quote! {
                if s.eq_ignore_ascii_case(#value) { return Ok(#enum_ident::#variant_ident); }
            });
        }
        enums.extend(quote! {
            #[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash, serde::Deserialize, serde::Serialize)]
            #[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
            pub enum #enum_ident {
                #variants
            }
            impl ConfigEnum for #enum_ident {
                #[inline]
                fn variants() -> &'static [Self] {
                    static VARIANTS: &[#enum_ident] = &[#full_variants];
                    VARIANTS
                }
                #[inline]
                fn variant_info() -> &'static [ConfigEnumVariantInfo] {
                    static VARIANT_INFO: &[ConfigEnumVariantInfo] = &[
                        #variant_info
                    ];
                    VARIANT_INFO
                }
                fn as_str(&self) -> &'static str {
                    match self {
                        #variant_to_str
                    }
                }
                fn name(&self) -> &'static str {
                    match self {
                        #variant_to_name
                    }
                }
                fn description(&self) -> Option<&'static str> {
                    match self {
                        #variant_to_description
                    }
                }
            }
            impl std::str::FromStr for #enum_ident {
                type Err = ();
                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    #variant_from_str
                    Err(())
                }
            }
        });
    }

    let mut groups = TokenStream::new();
    let mut group_idents = Vec::new();
    for group in &schema.groups {
        let ident = format_ident!("CONFIG_GROUP_{}", group.id.to_shouty_snake_case());
        let id = &group.id;
        let name = &group.name;
        let description = if let Some(description) = &group.description {
            quote! { Some(#description) }
        } else {
            quote! { None }
        };
        let properties =
            group.properties.iter().map(|p| format_ident!("{}", p.to_upper_camel_case()));
        groups.extend(quote! {
            ConfigPropertyGroup {
                id: #id,
                name: #name,
                description: #description,
                properties: &[#(ConfigPropertyId::#properties,)*],
            },
        });
        group_idents.push(ident);
    }

    let mut property_idents = Vec::new();
    let mut property_variants = TokenStream::new();
    let mut variant_info = TokenStream::new();
    let mut config_property_id_to_str = TokenStream::new();
    let mut config_property_id_to_name = TokenStream::new();
    let mut config_property_id_to_description = TokenStream::new();
    let mut config_property_id_to_kind = TokenStream::new();
    let mut property_fields = TokenStream::new();
    let mut default_fields = TokenStream::new();
    let mut get_property_value_variants = TokenStream::new();
    let mut set_property_value_variants = TokenStream::new();
    let mut set_property_value_str_variants = TokenStream::new();
    let mut config_property_id_from_str = TokenStream::new();
    for property in &schema.properties {
        let base = match property {
            ConfigProperty::Boolean(b) => &b.base,
            ConfigProperty::Choice(c) => &c.base,
        };
        let id = &base.id;
        let enum_ident = format_ident!("{}", id.to_upper_camel_case());
        property_idents.push(enum_ident.clone());
        config_property_id_to_str.extend(quote! { Self::#enum_ident => #id, });
        let name = &base.name;
        config_property_id_to_name.extend(quote! { Self::#enum_ident => #name, });
        if let Some(description) = &base.description {
            config_property_id_to_description.extend(quote! {
                Self::#enum_ident => Some(#description),
            });
        } else {
            config_property_id_to_description.extend(quote! {
                Self::#enum_ident => None,
            });
        }
        let doc = build_doc(name, base.description.as_deref());
        property_variants.extend(quote! { #doc #enum_ident, });
        property_fields.extend(doc);
        let field_ident = format_ident!("{}", id.to_snake_case());
        match property {
            ConfigProperty::Boolean(b) => {
                let default = b.default;
                if default {
                    property_fields.extend(quote! {
                        #[serde(default = "default_true")]
                    });
                }
                property_fields.extend(quote! {
                    pub #field_ident: bool,
                });
                default_fields.extend(quote! {
                    #field_ident: #default,
                });
            }
            ConfigProperty::Choice(_) => {
                property_fields.extend(quote! {
                    pub #field_ident: #enum_ident,
                });
                default_fields.extend(quote! {
                    #field_ident: #enum_ident::default(),
                });
            }
        }
        let property_value = match property {
            ConfigProperty::Boolean(_) => {
                quote! { ConfigPropertyValue::Boolean(self.#field_ident) }
            }
            ConfigProperty::Choice(_) => {
                quote! { ConfigPropertyValue::Choice(self.#field_ident.as_str()) }
            }
        };
        get_property_value_variants.extend(quote! {
            ConfigPropertyId::#enum_ident => #property_value,
        });
        match property {
            ConfigProperty::Boolean(_) => {
                set_property_value_variants.extend(quote! {
                    ConfigPropertyId::#enum_ident => {
                        if let ConfigPropertyValue::Boolean(value) = value {
                            self.#field_ident = value;
                            Ok(())
                        } else {
                            Err(())
                        }
                    },
                });
                set_property_value_str_variants.extend(quote! {
                    ConfigPropertyId::#enum_ident => {
                        if let Ok(value) = value.parse() {
                            self.#field_ident = value;
                            Ok(())
                        } else {
                            Err(())
                        }
                    },
                });
            }
            ConfigProperty::Choice(_) => {
                set_property_value_variants.extend(quote! {
                    ConfigPropertyId::#enum_ident => {
                        if let ConfigPropertyValue::Choice(value) = value {
                            if let Ok(value) = value.parse() {
                                self.#field_ident = value;
                                Ok(())
                            } else {
                                Err(())
                            }
                        } else {
                            Err(())
                        }
                    },
                });
                set_property_value_str_variants.extend(quote! {
                    ConfigPropertyId::#enum_ident => {
                        if let Ok(value) = value.parse() {
                            self.#field_ident = value;
                            Ok(())
                        } else {
                            Err(())
                        }
                    },
                });
            }
        }
        let description = if let Some(description) = &base.description {
            quote! { Some(#description) }
        } else {
            quote! { None }
        };
        variant_info.extend(quote! {
            ConfigEnumVariantInfo {
                value: #id,
                name: #name,
                description: #description,
                is_default: false,
            },
        });
        match property {
            ConfigProperty::Boolean(_) => {
                config_property_id_to_kind.extend(quote! {
                    Self::#enum_ident => ConfigPropertyKind::Boolean,
                });
            }
            ConfigProperty::Choice(_) => {
                config_property_id_to_kind.extend(quote! {
                    Self::#enum_ident => ConfigPropertyKind::Choice(#enum_ident::variant_info()),
                });
            }
        }
        let snake_id = id.to_snake_case();
        config_property_id_from_str.extend(quote! {
            if s.eq_ignore_ascii_case(#id) || s.eq_ignore_ascii_case(#snake_id) {
                return Ok(Self::#enum_ident);
            }
        });
    }

    let tokens = quote! {
        pub trait ConfigEnum: Sized {
            fn variants() -> &'static [Self];
            fn variant_info() -> &'static [ConfigEnumVariantInfo];
            fn as_str(&self) -> &'static str;
            fn name(&self) -> &'static str;
            fn description(&self) -> Option<&'static str>;
        }
        #[derive(Clone, Debug)]
        pub struct ConfigEnumVariantInfo {
            pub value: &'static str,
            pub name: &'static str,
            pub description: Option<&'static str>,
            pub is_default: bool,
        }
        #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
        pub enum ConfigPropertyId {
            #property_variants
        }
        impl ConfigEnum for ConfigPropertyId {
            #[inline]
            fn variants() -> &'static [Self] {
                static VARIANTS: &[ConfigPropertyId] = &[#(ConfigPropertyId::#property_idents,)*];
                VARIANTS
            }
            #[inline]
            fn variant_info() -> &'static [ConfigEnumVariantInfo] {
                static VARIANT_INFO: &[ConfigEnumVariantInfo] = &[
                    #variant_info
                ];
                VARIANT_INFO
            }
            fn as_str(&self) -> &'static str {
                match self {
                    #config_property_id_to_str
                }
            }
            fn name(&self) -> &'static str {
                match self {
                    #config_property_id_to_name
                }
            }
            fn description(&self) -> Option<&'static str> {
                match self {
                    #config_property_id_to_description
                }
            }
        }
        impl ConfigPropertyId {
            pub fn kind(&self) -> ConfigPropertyKind {
                match self {
                    #config_property_id_to_kind
                }
            }
        }
        impl std::str::FromStr for ConfigPropertyId {
            type Err = ();
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                #config_property_id_from_str
                Err(())
            }
        }
        #[derive(Clone, Debug)]
        pub struct ConfigPropertyGroup {
            pub id: &'static str,
            pub name: &'static str,
            pub description: Option<&'static str>,
            pub properties: &'static [ConfigPropertyId],
        }
        pub static CONFIG_GROUPS: &[ConfigPropertyGroup] = &[#groups];
        #[derive(Clone, Debug, Eq, PartialEq, Hash)]
        pub enum ConfigPropertyValue {
            Boolean(bool),
            Choice(&'static str),
        }
        impl ConfigPropertyValue {
            pub fn to_json(&self) -> serde_json::Value {
                match self {
                    ConfigPropertyValue::Boolean(value) => serde_json::Value::Bool(*value),
                    ConfigPropertyValue::Choice(value) => serde_json::Value::String(value.to_string()),
                }
            }
        }
        #[derive(Clone, Debug)]
        pub enum ConfigPropertyKind {
            Boolean,
            Choice(&'static [ConfigEnumVariantInfo]),
        }
        #enums
        #[inline(always)]
        fn default_true() -> bool { true }
        #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
        #[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
        #[serde(default)]
        pub struct DiffObjConfig {
            #property_fields
        }
        impl Default for DiffObjConfig {
            fn default() -> Self {
                Self {
                    #default_fields
                }
            }
        }
        impl DiffObjConfig {
            pub fn get_property_value(&self, id: ConfigPropertyId) -> ConfigPropertyValue {
                match id {
                    #get_property_value_variants
                }
            }
            #[allow(clippy::result_unit_err)]
            pub fn set_property_value(&mut self, id: ConfigPropertyId, value: ConfigPropertyValue) -> Result<(), ()> {
                match id {
                    #set_property_value_variants
                }
            }
            #[allow(clippy::result_unit_err)]
            pub fn set_property_value_str(&mut self, id: ConfigPropertyId, value: &str) -> Result<(), ()> {
                match id {
                    #set_property_value_str_variants
                }
            }
        }
    };
    let file = syn::parse2(tokens).unwrap();
    let formatted = prettyplease::unparse(&file);
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("config.gen.rs"),
        formatted,
    )
    .unwrap();
}
