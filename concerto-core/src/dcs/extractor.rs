//! `DecoratorExtractor`: a port of `src/decoratorextractor.ts`. Runs
//! `DecoratorManager`'s command-set model in reverse — walking a loaded
//! model's AST, pulling every decorator (or just its vocabulary — `Term`/
//! `Term_*` — decorators, or just its non-vocabulary ones) off each
//! declaration, property and map key/value, and building a
//! `DecoratorCommandSet` and a vocabulary YAML string that would recreate
//! them.
//!
//! Like [`super`], this stays on the untyped metamodel AST
//! ([`serde_json::Value`]) throughout, matching the reference.

use serde_json::{Map, Value};

// A decorator argument's value reads in the vocabulary YAML this module
// hand-builds as JS template-literal interpolation writes it, `String(value)`:
// [`to_js_string`], so a number is written the way
// `Number.prototype.toString()` writes it (P5-98, C-11).
use crate::ecma::to_js_string;
use crate::error::{ContractError, ErrorKind, Result};
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

use super::{MAP_DECLARATION_CLASS, quote_string_value};
use crate::instance::metamodel::metamodel_class;

/// `DecoratorExtractor.Action` (`src/decoratorextractor.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    /// Extract every decorator.
    #[default]
    ExtractAll,
    /// Extract only vocabulary (`Term`/`Term_*`) decorators.
    ExtractVocab,
    /// Extract only non-vocabulary decorators.
    ExtractNonVocab,
}

/// One AST node's collected decorators, keyed in the extraction dictionary
/// ([`ExtractionDictionary`]) by the namespace they were found in.
/// `ExtractedDecorator` (`src/decoratorextractor.ts`), borrowed from the
/// models being walked (P5-40): the names are the AST's own strings (empty
/// where TS's field is unset) and `decorators` is the AST `decorators`
/// array itself, read before any of it is stripped — TS's `dcs` field is a
/// `JSON.stringify`'d copy taken at the same point (`obj.dcs`, later
/// `JSON.parse`'d back in `transformDecoratorsAndVocabularies`), which a
/// borrow of the unchanged array reads the same as.
#[derive(Debug, Clone, Copy, Default)]
struct ExtractedDecorators<'a> {
    declaration: &'a str,
    property: &'a str,
    map_element: &'a str,
    decorators: &'a [Value],
}

/// `this.extractionDictionary` (`src/decoratorextractor.ts`), a JS object
/// keyed by namespace: kept in insertion order, the order `Object.keys`
/// then walks it in.
type ExtractionDictionary<'a> = Vec<(&'a str, Vec<ExtractedDecorators<'a>>)>;

/// The result of [`DecoratorExtractor::extract`]: `ExtractDecoratorsResult`
/// (`src/decoratormanager.ts`'s JSDoc typedef).
pub struct ExtractResult {
    /// A model manager over the (possibly decorator-stripped) models.
    pub model_manager: ModelManager,
    /// The extracted, non-vocabulary decorators, as `DecoratorCommandSet`
    /// JSON objects (one per namespace that had any).
    pub decorator_command_set: Vec<Value>,
    /// The extracted vocabulary (`Term`/`Term_*`) decorators, as vocabulary
    /// YAML strings (one per namespace that had any).
    pub vocabularies: Vec<String>,
}

/// The result of [`DecoratorExtractor::extract_encoded`]: an
/// [`ExtractResult`] with its command sets already encoded as JSON text
/// (P5-57, T3, accordproject/concerto-rust#378).
pub struct EncodedExtractResult {
    /// A model manager over the (possibly decorator-stripped) models.
    pub model_manager: ModelManager,
    /// The JSON text of [`ExtractResult::decorator_command_set`], the
    /// array, byte for byte what `serde_json::to_string` of that
    /// `Vec<Value>` gives.
    pub decorator_command_set: String,
    /// [`ExtractResult::vocabularies`].
    pub vocabularies: Vec<String>,
}

/// The `$class` strings every command a [`DecoratorExtractor`] builds
/// repeats, formatted once per extraction rather than once per decorator.
struct CommandClasses {
    command: String,
    target: String,
    decorator: &'static str,
    type_reference: &'static str,
}

/// `DecoratorExtractor` (`src/decoratorextractor.ts`).
pub struct DecoratorExtractor {
    remove_decorators_from_model: bool,
    locale: String,
    dcs_version: String,
    updated_model_ast: Value,
    action: Action,
}

impl DecoratorExtractor {
    /// `new DecoratorExtractor(...)` (`src/decoratorextractor.ts`).
    /// `source_model_ast` is `IModels`-shaped: `{ "$class": "...Models",
    /// "models": [...] }`.
    pub fn new(
        remove_decorators_from_model: bool,
        locale: impl Into<String>,
        dcs_version: impl Into<String>,
        source_model_ast: Value,
        action: Action,
    ) -> Self {
        Self {
            remove_decorators_from_model,
            locale: locale.into(),
            dcs_version: dcs_version.into(),
            updated_model_ast: source_model_ast,
            action,
        }
    }

    /// `DecoratorExtractor.isVocabDecorator` (`src/decoratorextractor.ts`).
    fn is_vocab_decorator(name: &str) -> bool {
        name == "Term" || name.starts_with("Term_")
    }

    /// A decorator node's `name`, `""` when it has none (TS's
    /// `isVocabDecorator(dcs.name)` on `undefined` is false, and so is
    /// this on `""`).
    fn decorator_name(decorator: &Value) -> &str {
        decorator
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// `DecoratorExtractor.transformNonVocabularyDecorators`
    /// (`src/decoratorextractor.ts`).
    fn transform_non_vocabulary_decorators(
        &self,
        dcs_objects: Vec<Value>,
        namespace: &str,
        decorator_data: &mut Vec<Value>,
    ) -> Result<()> {
        if dcs_objects.is_empty() {
            return Ok(());
        }
        let ParsedNamespace::Full { name, version, .. } =
            model_util::parse_namespace_with(Some(namespace), false)?
        else {
            unreachable!("parse_namespace_with(_, false) always returns Full")
        };
        let mut m = Map::new();
        m.insert(
            "$class".to_string(),
            Value::String(format!(
                "org.accordproject.decoratorcommands@{}.DecoratorCommandSet",
                self.dcs_version
            )),
        );
        m.insert("name".to_string(), Value::String(name));
        m.insert(
            "version".to_string(),
            version.map_or(Value::Null, Value::String),
        );
        m.insert("commands".to_string(), Value::Array(dcs_objects));
        decorator_data.push(Value::Object(m));
        Ok(())
    }

    /// `DecoratorExtractor.transformVocabularyDecorators`
    /// (`src/decoratorextractor.ts`).
    fn transform_vocabulary_decorators(
        &self,
        vocab_object: &Value,
        namespace: &str,
        vocab_data: &mut Vec<String>,
    ) {
        let Some(obj) = vocab_object.as_object() else {
            return;
        };
        if obj.is_empty() {
            return;
        }
        let mut s = String::new();
        s.push_str(&format!("locale: {}\n", self.locale));
        s.push_str(&format!("namespace: {namespace}\n"));

        if let Some(ns_obj) = vocab_object.get("namespace").and_then(Value::as_object)
            && !ns_obj.is_empty()
        {
            if let Some(term) = ns_obj.get("term") {
                s.push_str(&format!("term: {}\n", to_js_string(term)));
            }
            for (key, value) in ns_obj {
                if key != "term" {
                    s.push_str(&format!("{key}: {}\n", to_js_string(value)));
                }
            }
        }

        if let Some(decls) = vocab_object.get("declarations").and_then(Value::as_object)
            && !decls.is_empty()
        {
            s.push_str("declarations:\n");
            for (decl_name, decl) in decls {
                let Some(decl_obj) = decl.as_object() else {
                    continue;
                };
                let has_term = decl_obj.contains_key("term");
                if has_term {
                    s.push_str(&format!(
                        "  - {decl_name}: {}\n",
                        to_js_string(&decl_obj["term"])
                    ));
                }
                let other_props: Vec<&String> = decl_obj
                    .keys()
                    .filter(|k| k.as_str() != "term" && k.as_str() != "propertyVocabs")
                    .collect();
                // If a declaration has no Term decorator, add Term_ decorators to the YAML.
                if !other_props.is_empty() {
                    if !has_term {
                        s.push_str(&format!("  - {decl_name}: {decl_name}\n"));
                    }
                    for key in &other_props {
                        s.push_str(&format!(
                            "    {key}: {}\n",
                            to_js_string(&decl_obj[key.as_str()])
                        ));
                    }
                }
                if let Some(prop_vocabs) = decl_obj.get("propertyVocabs").and_then(Value::as_object)
                    && !prop_vocabs.is_empty()
                {
                    if !has_term && other_props.is_empty() {
                        s.push_str(&format!("  - {decl_name}: {decl_name}\n"));
                    }
                    s.push_str("    properties:\n");
                    for (prop, prop_vocab) in prop_vocabs {
                        let Some(prop_obj) = prop_vocab.as_object() else {
                            continue;
                        };
                        let term_val = prop_obj
                            .get("term")
                            .map(to_js_string)
                            .unwrap_or_else(|| prop.clone());
                        s.push_str(&format!("      - {prop}: {term_val}\n"));
                        for (key, value) in prop_obj {
                            if key != "term" {
                                s.push_str(&format!("        {key}: {}\n", to_js_string(value)));
                            }
                        }
                    }
                }
            }
        } else {
            s.push_str("declarations: []\n");
        }
        vocab_data.push(s);
    }

    /// `DecoratorExtractor.constructTarget` (`src/decoratorextractor.ts`).
    fn construct_target(
        classes: &CommandClasses,
        namespace: &str,
        obj: &ExtractedDecorators<'_>,
    ) -> Value {
        let mut m = Map::new();
        m.insert("$class".to_string(), Value::String(classes.target.clone()));
        m.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        if !obj.declaration.is_empty() {
            m.insert(
                "declaration".to_string(),
                Value::String(obj.declaration.to_string()),
            );
        }
        if !obj.property.is_empty() {
            m.insert(
                "property".to_string(),
                Value::String(obj.property.to_string()),
            );
        }
        if !obj.map_element.is_empty() {
            m.insert(
                "mapElement".to_string(),
                Value::String(obj.map_element.to_string()),
            );
        }
        Value::Object(m)
    }

    /// `DecoratorExtractor.parseNonVocabularyDecorators`
    /// (`src/decoratorextractor.ts`).
    fn parse_non_vocabulary_decorators(
        dcs_objects: &mut Vec<Value>,
        dcs: &Value,
        classes: &CommandClasses,
        target: &Value,
    ) {
        let mut decorator = Map::new();
        decorator.insert(
            "$class".to_string(),
            Value::String(classes.decorator.to_string()),
        );
        decorator.insert(
            "name".to_string(),
            dcs.get("name").cloned().unwrap_or(Value::Null),
        );
        if let Some(args) = dcs.get("arguments").and_then(Value::as_array) {
            let ported_args: Vec<Value> = args
                .iter()
                .map(|arg| {
                    let mut m = Map::new();
                    let class = arg.get("$class").cloned().unwrap_or(Value::Null);
                    let is_type_reference = class.as_str() == Some(classes.type_reference);
                    m.insert("$class".to_string(), class);
                    if is_type_reference {
                        m.insert(
                            "type".to_string(),
                            arg.get("type").cloned().unwrap_or(Value::Null),
                        );
                        m.insert(
                            "isArray".to_string(),
                            arg.get("isArray").cloned().unwrap_or(Value::Null),
                        );
                    } else {
                        m.insert(
                            "value".to_string(),
                            arg.get("value").cloned().unwrap_or(Value::Null),
                        );
                    }
                    Value::Object(m)
                })
                .collect();
            decorator.insert("arguments".to_string(), Value::Array(ported_args));
        }

        let mut command = Map::new();
        command.insert("$class".to_string(), Value::String(classes.command.clone()));
        command.insert("type".to_string(), Value::String("UPSERT".to_string()));
        command.insert("target".to_string(), target.clone());
        command.insert("decorator".to_string(), Value::Object(decorator));
        dcs_objects.push(Value::Object(command));
    }

    /// The quoted YAML text of a vocabulary decorator's first argument, as
    /// `parseVocabularies` (`src/decoratorextractor.ts`) writes it.
    fn vocab_argument(dcs: &Value) -> String {
        let arg0 = dcs.get("arguments").and_then(|a| a.get(0));
        let arg_value = arg0
            .and_then(|a| a.get("value"))
            .map(to_js_string)
            .unwrap_or_default();
        let arg_class = arg0.and_then(|a| a.get("$class")).and_then(Value::as_str);
        quote_string_value(&arg_value, arg_class)
    }

    /// `parseVocabularies`' error for a namespace-level `Term_` key that is
    /// one of the reserved YAML keys.
    fn reserved_namespace_key(extension_key: &str) -> crate::error::Error {
        ContractError::pre_port(
            ErrorKind::InvalidArgument,
            format!("Invalid vocabulary key: {extension_key}. The key should not be one of the reserved keys: namespace, locale, declarations"),
            None,
        )
        .into()
    }

    /// `parseVocabularies`' error for a property- or map-element-level
    /// `Term_` key named after that property (or map element).
    fn reserved_property_key(extension_key: &str) -> crate::error::Error {
        ContractError::pre_port(
            ErrorKind::InvalidArgument,
            format!("Invalid vocabulary key: \"{extension_key}\". The key should not be the name of the current property."),
            None,
        )
        .into()
    }

    /// `parseVocabularies`' error for a declaration-level `Term_` key that
    /// is `properties` or the declaration's own name.
    fn reserved_declaration_key(extension_key: &str) -> crate::error::Error {
        ContractError::pre_port(
            ErrorKind::InvalidArgument,
            format!("Invalid vocabulary key: \"{extension_key}\". The key cannot be a reserved word such as \"properties\" or the name of the current declaration."),
            None,
        )
        .into()
    }

    /// `DecoratorExtractor.parseVocabularies` (`src/decoratorextractor.ts`).
    fn parse_vocabularies(
        vocab_object: &mut Value,
        vocab_target: &ExtractedDecorators<'_>,
        dcs: &Value,
    ) -> Result<()> {
        if !vocab_object.is_object() {
            *vocab_object = Value::Object(Map::new());
        }
        let dcs_name = Self::decorator_name(dcs);
        let quoted = Self::vocab_argument(dcs);

        if vocab_target.declaration.is_empty() {
            let ns = ensure_object(vocab_object, "namespace");
            if dcs_name == "Term" {
                ns.insert("term".to_string(), Value::String(quoted));
            } else {
                let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);
                if matches!(extension_key, "namespace" | "locale" | "declarations") {
                    return Err(Self::reserved_namespace_key(extension_key));
                }
                ns.insert(extension_key.to_string(), Value::String(quoted));
            }
            return Ok(());
        }

        let declarations = ensure_object(vocab_object, "declarations");
        let decl_entry = declarations
            .entry(vocab_target.declaration)
            .or_insert_with(|| {
                let mut m = Map::new();
                m.insert("propertyVocabs".to_string(), Value::Object(Map::new()));
                Value::Object(m)
            });
        if !vocab_target.property.is_empty() {
            let prop_vocabs = ensure_object(decl_entry, "propertyVocabs");
            let prop_vocab = prop_vocabs
                .entry(vocab_target.property)
                .or_insert_with(|| Value::Object(Map::new()));
            let prop_vocab = prop_vocab.as_object_mut().expect("just ensured object");
            if dcs_name == "Term" {
                prop_vocab.insert("term".to_string(), Value::String(quoted));
            } else {
                let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);
                if extension_key == vocab_target.property {
                    return Err(Self::reserved_property_key(extension_key));
                }
                prop_vocab.insert(extension_key.to_string(), Value::String(quoted));
            }
        } else if !vocab_target.map_element.is_empty() {
            let prop_vocabs = ensure_object(decl_entry, "propertyVocabs");
            let map_vocab = prop_vocabs
                .entry(vocab_target.map_element)
                .or_insert_with(|| Value::Object(Map::new()));
            let map_vocab = map_vocab.as_object_mut().expect("just ensured object");
            if dcs_name == "Term" {
                map_vocab.insert("term".to_string(), Value::String(quoted));
            } else {
                let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);
                if extension_key == vocab_target.map_element {
                    return Err(Self::reserved_property_key(extension_key));
                }
                map_vocab.insert(extension_key.to_string(), Value::String(quoted));
            }
        } else {
            let decl_obj = decl_entry
                .as_object_mut()
                .expect("declarations entries are objects");
            if dcs_name == "Term" {
                decl_obj.insert("term".to_string(), Value::String(quoted));
            } else {
                let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);
                if extension_key == "properties" || extension_key == vocab_target.declaration {
                    return Err(Self::reserved_declaration_key(extension_key));
                }
                decl_obj.insert(extension_key.to_string(), Value::String(quoted));
            }
        }
        Ok(())
    }

    /// The `$class` strings of this extraction's commands.
    fn command_classes(&self) -> CommandClasses {
        let version = &self.dcs_version;
        CommandClasses {
            command: format!("org.accordproject.decoratorcommands@{version}.Command"),
            target: format!("org.accordproject.decoratorcommands@{version}.CommandTarget"),
            decorator: metamodel_class!("Decorator"),
            type_reference: metamodel_class!("DecoratorTypeReference"),
        }
    }

    /// P5-57 (T3, accordproject/concerto-rust#378): what
    /// [`Self::transform_decorators_and_vocabularies`] computes, built
    /// straight from the borrowed dictionary without any intermediate
    /// [`Value`]: the command sets as the JSON text of the array that
    /// `serde_json::to_string` of the `Value` route's `Vec<Value>` gives
    /// (serialised through borrowed views of the AST nodes, [`CommandSetView`]),
    /// and the vocabularies from a borrowed tree ([`VocabTree`]) in place of
    /// the `vocabObject` `Value`. Same walk, same order, same first error.
    fn encode_decorators_and_vocabularies(
        &self,
        extraction_dictionary: &ExtractionDictionary<'_>,
    ) -> Result<(String, Vec<String>)> {
        let classes = self.command_classes();
        let set_class = format!(
            "org.accordproject.decoratorcommands@{}.DecoratorCommandSet",
            self.dcs_version
        );
        let mut command_sets = Vec::new();
        let mut vocab_data = Vec::new();
        for (namespace, entries) in extraction_dictionary {
            let mut commands = Vec::new();
            let mut vocab = VocabTree::default();
            for entry in entries {
                for dcs in entry.decorators {
                    let is_vocab = Self::is_vocab_decorator(Self::decorator_name(dcs));
                    if !is_vocab && self.action != Action::ExtractVocab {
                        commands.push(CommandView {
                            classes: &classes,
                            namespace,
                            target: entry,
                            decorator: dcs,
                        });
                    }
                    if is_vocab && self.action != Action::ExtractNonVocab {
                        vocab.parse(entry, dcs)?;
                    }
                }
            }
            if self.action != Action::ExtractVocab && !commands.is_empty() {
                let ParsedNamespace::Full { name, version, .. } =
                    model_util::parse_namespace_with(Some(namespace), false)?
                else {
                    unreachable!("parse_namespace_with(_, false) always returns Full")
                };
                command_sets.push(CommandSetView {
                    class: &set_class,
                    name,
                    version,
                    commands,
                });
            }
            if self.action != Action::ExtractNonVocab
                && let Some(yaml) = vocab.to_yaml(&self.locale, namespace)
            {
                vocab_data.push(yaml);
            }
        }
        let text = serde_json::to_string(&command_sets).map_err(|e| {
            crate::error::Error::from(ContractError::pre_port(
                ErrorKind::MalformedInput,
                format!("the decorator command sets could not be encoded: {e}"),
                None,
            ))
        })?;
        Ok((text, vocab_data))
    }

    /// `DecoratorExtractor.transformDecoratorsAndVocabularies`
    /// (`src/decoratorextractor.ts`), over the borrowed dictionary
    /// [`collect_models`] built.
    fn transform_decorators_and_vocabularies(
        &self,
        extraction_dictionary: &ExtractionDictionary<'_>,
    ) -> Result<(Vec<Value>, Vec<String>)> {
        let classes = self.command_classes();
        let mut decorator_data = Vec::new();
        let mut vocab_data = Vec::new();
        for (namespace, entries) in extraction_dictionary {
            let mut dcs_objects = Vec::new();
            let mut vocab_object = Value::Object(Map::new());
            for entry in entries {
                // TS builds the target for every entry; it is only read by
                // the non-vocabulary commands, so it is built for the first.
                let mut target = None;
                for dcs in entry.decorators {
                    let is_vocab = Self::is_vocab_decorator(Self::decorator_name(dcs));
                    if !is_vocab && self.action != Action::ExtractVocab {
                        let target = target.get_or_insert_with(|| {
                            Self::construct_target(&classes, namespace, entry)
                        });
                        Self::parse_non_vocabulary_decorators(
                            &mut dcs_objects,
                            dcs,
                            &classes,
                            target,
                        );
                    }
                    if is_vocab && self.action != Action::ExtractNonVocab {
                        Self::parse_vocabularies(&mut vocab_object, entry, dcs)?;
                    }
                }
            }
            if self.action != Action::ExtractVocab {
                self.transform_non_vocabulary_decorators(
                    dcs_objects,
                    namespace,
                    &mut decorator_data,
                )?;
            }
            if self.action != Action::ExtractNonVocab {
                self.transform_vocabulary_decorators(&vocab_object, namespace, &mut vocab_data);
            }
        }
        Ok((decorator_data, vocab_data))
    }

    /// `DecoratorExtractor.filterOutDecorators` (`src/decoratorextractor.ts`),
    /// applied in place to `node`'s own `decorators` array, when it has
    /// one: with `removeDecoratorsFromModel` unset TS writes the same array
    /// back, so nothing changes; otherwise `EXTRACT_ALL` deletes the key and
    /// the other actions keep only the decorators they do not extract.
    fn filter_out_decorators(&self, node: &mut Value) {
        if !self.remove_decorators_from_model {
            return;
        }
        let Some(map) = node.as_object_mut() else {
            return;
        };
        let keep_vocab = match self.action {
            Action::ExtractAll => {
                if matches!(map.get("decorators"), Some(Value::Array(_))) {
                    map.remove("decorators");
                }
                return;
            }
            Action::ExtractVocab => false,
            Action::ExtractNonVocab => true,
        };
        if let Some(Value::Array(decorators)) = map.get_mut("decorators") {
            decorators.retain(|d| Self::is_vocab_decorator(Self::decorator_name(d)) == keep_vocab);
        }
    }

    /// The model-changing half of `DecoratorExtractor.processModels`
    /// (`processDeclarations`, `processMapDeclaration`, `processProperties`,
    /// `src/decoratorextractor.ts`), run after [`collect_models`] has read
    /// every decorator: each node's decorators filtered
    /// ([`Self::filter_out_decorators`]), and a model with no `declarations`
    /// given an empty array, as TS's `model.declarations = ...map(...)`
    /// leaves it.
    fn process_models(&self, models: &mut [Value]) {
        for model in models.iter_mut() {
            if has_decorators(model) {
                self.filter_out_decorators(model);
            }
            let Some(model_map) = model.as_object_mut() else {
                continue;
            };
            let declarations = model_map
                .entry("declarations")
                .or_insert_with(|| Value::Array(Vec::new()));
            if !self.remove_decorators_from_model {
                continue;
            }
            let Value::Array(declarations) = declarations else {
                continue;
            };
            for decl in declarations.iter_mut() {
                self.filter_out_decorators(decl);
                if decl.get("$class").and_then(Value::as_str) == Some(MAP_DECLARATION_CLASS)
                    && let Some(map) = decl.as_object_mut()
                {
                    if let Some(key) = map.get_mut("key") {
                        self.filter_out_decorators(key);
                    }
                    if let Some(value) = map.get_mut("value") {
                        self.filter_out_decorators(value);
                    }
                }
                if let Some(Value::Array(properties)) = decl.get_mut("properties") {
                    for property in properties.iter_mut() {
                        self.filter_out_decorators(property);
                    }
                }
            }
        }
    }

    /// `DecoratorExtractor.extract` (`src/decoratorextractor.ts`).
    ///
    /// P5-40 (F-B): the models are walked twice rather than once. The first
    /// walk ([`collect_models`]) only borrows them, recording where each
    /// `decorators` array is; the command sets and vocabularies are built
    /// from those borrows before the second walk
    /// ([`Self::process_models`]) strips the decorators in place, and the
    /// models are then moved, not copied, into the result manager. Errors
    /// keep TS's order: a load or validation failure of the result models
    /// is thrown ahead of a vocabulary-key error from the transform.
    pub fn extract(self) -> Result<ExtractResult> {
        let (model_manager, (decorator_command_set, vocabularies)) =
            self.extract_with(Self::transform_decorators_and_vocabularies)?;
        Ok(ExtractResult {
            model_manager,
            decorator_command_set,
            vocabularies,
        })
    }

    /// [`Self::extract`] with the command sets and vocabularies encoded
    /// directly from the borrowed AST nodes (P5-57, T3,
    /// accordproject/concerto-rust#378;
    /// [`Self::encode_decorators_and_vocabularies`]): the same result, the
    /// same errors in the same order, with the command sets as JSON text.
    pub fn extract_encoded(self) -> Result<EncodedExtractResult> {
        let (model_manager, (decorator_command_set, vocabularies)) =
            self.extract_with(Self::encode_decorators_and_vocabularies)?;
        Ok(EncodedExtractResult {
            model_manager,
            decorator_command_set,
            vocabularies,
        })
    }

    /// [`Self::extract_encoded`], also returning a copy of the source models
    /// its walk read (P5-56, T2, F-A2, accordproject/concerto-rust#377):
    /// taken before the walk, so [`Self::encode_source`] over them gives
    /// exactly this call's command sets and vocabularies. The same result
    /// and the same errors, in the same order, as [`Self::extract_encoded`].
    pub fn extract_encoded_keeping_source(self) -> Result<(EncodedExtractResult, Vec<Value>)> {
        let (model_manager, (decorator_command_set, vocabularies), source) =
            self.extract_with_source(true, Self::encode_decorators_and_vocabularies)?;
        Ok((
            EncodedExtractResult {
                model_manager,
                decorator_command_set,
                vocabularies,
            },
            source.unwrap_or_default(),
        ))
    }

    /// The command sets (as JSON text) and vocabularies that
    /// [`Self::extract_encoded`] would return for `models` (P5-56): the same
    /// walk and the same transform, with the same first error, but no
    /// result manager. `models` are the source models of an earlier
    /// extraction ([`Self::extract_encoded_keeping_source`]); this
    /// extractor's own source AST is not read. The command sets and
    /// vocabularies are read before any decorator is stripped, so
    /// `removeDecoratorsFromModel` does not change them.
    pub fn encode_source(&self, models: &[Value]) -> Result<(String, Vec<String>)> {
        let mut extraction_dictionary = ExtractionDictionary::new();
        collect_models(&mut extraction_dictionary, models);
        self.encode_decorators_and_vocabularies(&extraction_dictionary)
    }

    /// The body of [`Self::extract`] and [`Self::extract_encoded`], which
    /// differ only in how `transform` builds the command sets and
    /// vocabularies from the borrowed dictionary.
    fn extract_with<T>(
        self,
        transform: impl FnOnce(&Self, &ExtractionDictionary<'_>) -> Result<T>,
    ) -> Result<(ModelManager, T)> {
        let (model_manager, transformed, _) = self.extract_with_source(false, transform)?;
        Ok((model_manager, transformed))
    }

    /// [`Self::extract_with`], with a copy of the source models taken
    /// before the walk when `keep_source` is set (P5-56).
    fn extract_with_source<T>(
        mut self,
        keep_source: bool,
        transform: impl FnOnce(&Self, &ExtractionDictionary<'_>) -> Result<T>,
    ) -> Result<(ModelManager, T, Option<Vec<Value>>)> {
        let mut models = match self.updated_model_ast.get_mut("models").map(std::mem::take) {
            Some(Value::Array(models)) => models,
            _ => Vec::new(),
        };
        let source = keep_source.then(|| models.clone());
        let transformed = {
            let mut extraction_dictionary = ExtractionDictionary::new();
            collect_models(&mut extraction_dictionary, &models);
            transform(&self, &extraction_dictionary)
        };
        self.process_models(&mut models);

        // `new ModelManager()` then `fromAst(this.updatedModelAst)`: every
        // model but the system ones (already preloaded), then validated.
        let mut model_manager = ModelManager::new()?;
        for model in models.into_iter().filter(|m| {
            !m.get("namespace")
                .and_then(Value::as_str)
                .is_some_and(|ns| crate::model_manager::EXCLUDE_NS.contains(&ns))
        }) {
            model_manager.add_owned_model_with_definitions(model, None, None)?;
        }
        model_manager.validate_models()?;

        Ok((model_manager, transformed?, source))
    }
}

/// `null`, for a node's missing field, borrowed.
static NULL: Value = Value::Null;

/// P5-57: one `DecoratorCommandSet`, as
/// [`DecoratorExtractor::transform_non_vocabulary_decorators`] builds it
/// (same keys, same order), serialised from borrows.
struct CommandSetView<'a> {
    class: &'a str,
    name: String,
    version: Option<String>,
    commands: Vec<CommandView<'a>>,
}

impl serde::Serialize for CommandSetView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(4))?;
        map.serialize_entry("$class", self.class)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("version", &self.version)?;
        map.serialize_entry("commands", &self.commands)?;
        map.end()
    }
}

/// P5-57: one `UPSERT` command, as
/// [`DecoratorExtractor::parse_non_vocabulary_decorators`] builds it (with
/// [`DecoratorExtractor::construct_target`]'s target), serialised from the
/// borrowed decorator node.
struct CommandView<'a> {
    classes: &'a CommandClasses,
    namespace: &'a str,
    target: &'a ExtractedDecorators<'a>,
    decorator: &'a Value,
}

impl serde::Serialize for CommandView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(4))?;
        map.serialize_entry("$class", &self.classes.command)?;
        map.serialize_entry("type", "UPSERT")?;
        map.serialize_entry("target", &TargetView(self))?;
        map.serialize_entry("decorator", &DecoratorView(self))?;
        map.end()
    }
}

/// [`CommandView`]'s `target`: [`DecoratorExtractor::construct_target`].
struct TargetView<'a>(&'a CommandView<'a>);

impl serde::Serialize for TargetView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let command = self.0;
        let target = command.target;
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("$class", &command.classes.target)?;
        map.serialize_entry("namespace", command.namespace)?;
        if !target.declaration.is_empty() {
            map.serialize_entry("declaration", target.declaration)?;
        }
        if !target.property.is_empty() {
            map.serialize_entry("property", target.property)?;
        }
        if !target.map_element.is_empty() {
            map.serialize_entry("mapElement", target.map_element)?;
        }
        map.end()
    }
}

/// [`CommandView`]'s `decorator`: the decorator node's `name` and ported
/// `arguments` ([`DecoratorExtractor::parse_non_vocabulary_decorators`]).
struct DecoratorView<'a>(&'a CommandView<'a>);

impl serde::Serialize for DecoratorView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let command = self.0;
        let dcs = command.decorator;
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("$class", &command.classes.decorator)?;
        map.serialize_entry("name", dcs.get("name").unwrap_or(&NULL))?;
        if let Some(args) = dcs.get("arguments").and_then(Value::as_array) {
            let type_reference = command.classes.type_reference;
            map.serialize_entry(
                "arguments",
                &ArgumentsView {
                    args,
                    type_reference,
                },
            )?;
        }
        map.end()
    }
}

/// [`DecoratorView`]'s ported `arguments`.
struct ArgumentsView<'a> {
    args: &'a [Value],
    type_reference: &'a str,
}

impl serde::Serialize for ArgumentsView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_seq(self.args.iter().map(|arg| ArgumentView {
            arg,
            type_reference: self.type_reference,
        }))
    }
}

/// One ported argument: its `$class`, then `type` and `isArray` for a type
/// reference, `value` otherwise, each `null` where the node has none.
struct ArgumentView<'a> {
    arg: &'a Value,
    type_reference: &'a str,
}

impl serde::Serialize for ArgumentView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let arg = self.arg;
        let class = arg.get("$class").unwrap_or(&NULL);
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("$class", class)?;
        if class.as_str() == Some(self.type_reference) {
            map.serialize_entry("type", arg.get("type").unwrap_or(&NULL))?;
            map.serialize_entry("isArray", arg.get("isArray").unwrap_or(&NULL))?;
        } else {
            map.serialize_entry("value", arg.get("value").unwrap_or(&NULL))?;
        }
        map.end()
    }
}

/// The entry for `key` in an insertion-ordered list of borrowed keys, added
/// (with `V::default()`) at the end when it is missing: an `IndexMap`'s
/// `entry(key).or_insert_with(..)`, which the `Value` route's
/// `serde_json::Map` (`preserve_order`) is. A node's decorators all come
/// together in the walk, so the last entry is tried first.
fn entry_of<'s, 'a, V: Default>(entries: &'s mut Vec<(&'a str, V)>, key: &'a str) -> &'s mut V {
    let index = match entries.iter().rposition(|(k, _)| *k == key) {
        Some(index) => index,
        None => {
            entries.push((key, V::default()));
            entries.len() - 1
        }
    };
    &mut entries[index].1
}

/// One object of `vocabObject` whose values are all quoted strings (the
/// namespace's, a declaration's own keys, a property's or map element's):
/// its `term`, and its other keys in insertion order. Setting a key again
/// replaces its value in place, as `Map::insert` does.
#[derive(Default)]
struct VocabEntry<'a> {
    term: Option<String>,
    others: Vec<(&'a str, String)>,
}

impl<'a> VocabEntry<'a> {
    fn set(&mut self, key: &'a str, value: String) {
        if key == "term" {
            self.term = Some(value);
        } else {
            *entry_of(&mut self.others, key) = value;
        }
    }
}

/// One `vocabObject.declarations` entry: its own keys, and its
/// `propertyVocabs` object (`None` once a `Term_propertyVocabs` decorator
/// has overwritten it with a string, which the YAML never prints).
struct DeclarationVocab<'a> {
    entry: VocabEntry<'a>,
    property_vocabs: Option<Vec<(&'a str, VocabEntry<'a>)>>,
}

impl Default for DeclarationVocab<'_> {
    /// `{ propertyVocabs: {} }`, as `parseVocabularies` creates it.
    fn default() -> Self {
        Self {
            entry: VocabEntry::default(),
            property_vocabs: Some(Vec::new()),
        }
    }
}

/// P5-57: `vocabObject` (`src/decoratorextractor.ts`) for one namespace,
/// over borrowed keys, in place of the `Value` route's `serde_json::Value`
/// object. `namespace` and `declarations` are `None` until a decorator
/// creates them.
#[derive(Default)]
struct VocabTree<'a> {
    namespace: Option<VocabEntry<'a>>,
    declarations: Option<Vec<(&'a str, DeclarationVocab<'a>)>>,
}

impl<'a> VocabTree<'a> {
    /// [`DecoratorExtractor::parse_vocabularies`], into this tree.
    fn parse(&mut self, vocab_target: &ExtractedDecorators<'a>, dcs: &'a Value) -> Result<()> {
        let dcs_name = DecoratorExtractor::decorator_name(dcs);
        let quoted = DecoratorExtractor::vocab_argument(dcs);
        let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);

        if vocab_target.declaration.is_empty() {
            let ns = self.namespace.get_or_insert_with(VocabEntry::default);
            if dcs_name == "Term" {
                ns.set("term", quoted);
            } else {
                if matches!(extension_key, "namespace" | "locale" | "declarations") {
                    return Err(DecoratorExtractor::reserved_namespace_key(extension_key));
                }
                ns.set(extension_key, quoted);
            }
            return Ok(());
        }

        let declarations = self.declarations.get_or_insert_with(Vec::new);
        let decl = entry_of(declarations, vocab_target.declaration);
        let element = if !vocab_target.property.is_empty() {
            vocab_target.property
        } else {
            vocab_target.map_element
        };
        if !element.is_empty() {
            let property_vocabs = decl.property_vocabs.get_or_insert_with(Vec::new);
            let vocab = entry_of(property_vocabs, element);
            if dcs_name == "Term" {
                vocab.set("term", quoted);
            } else {
                if extension_key == element {
                    return Err(DecoratorExtractor::reserved_property_key(extension_key));
                }
                vocab.set(extension_key, quoted);
            }
        } else if dcs_name == "Term" {
            decl.entry.set("term", quoted);
        } else {
            if extension_key == "properties" || extension_key == vocab_target.declaration {
                return Err(DecoratorExtractor::reserved_declaration_key(extension_key));
            }
            if extension_key == "propertyVocabs" {
                decl.property_vocabs = None;
            } else {
                decl.entry.set(extension_key, quoted);
            }
        }
        Ok(())
    }

    /// [`DecoratorExtractor::transform_vocabulary_decorators`]: the YAML of
    /// this tree, `None` when no vocabulary decorator was found.
    fn to_yaml(&self, locale: &str, namespace: &str) -> Option<String> {
        use std::fmt::Write;
        if self.namespace.is_none() && self.declarations.is_none() {
            return None;
        }
        let mut s = String::new();
        // `fmt::Write` for `String` never fails.
        let _ = write!(s, "locale: {locale}\nnamespace: {namespace}\n");
        if let Some(ns) = &self.namespace {
            if let Some(term) = &ns.term {
                let _ = writeln!(s, "term: {term}");
            }
            for (key, value) in &ns.others {
                let _ = writeln!(s, "{key}: {value}");
            }
        }
        match &self.declarations {
            Some(declarations) if !declarations.is_empty() => {
                s.push_str("declarations:\n");
                for (decl_name, decl) in declarations {
                    let term = decl.entry.term.as_deref();
                    if let Some(term) = term {
                        let _ = writeln!(s, "  - {decl_name}: {term}");
                    }
                    let others = &decl.entry.others;
                    if !others.is_empty() {
                        if term.is_none() {
                            let _ = writeln!(s, "  - {decl_name}: {decl_name}");
                        }
                        for (key, value) in others {
                            let _ = writeln!(s, "    {key}: {value}");
                        }
                    }
                    if let Some(property_vocabs) = &decl.property_vocabs
                        && !property_vocabs.is_empty()
                    {
                        if term.is_none() && others.is_empty() {
                            let _ = writeln!(s, "  - {decl_name}: {decl_name}");
                        }
                        s.push_str("    properties:\n");
                        for (prop, vocab) in property_vocabs {
                            let term = vocab.term.as_deref().unwrap_or(prop);
                            let _ = writeln!(s, "      - {prop}: {term}");
                            for (key, value) in &vocab.others {
                                let _ = writeln!(s, "        {key}: {value}");
                            }
                        }
                    }
                }
            }
            _ => s.push_str("declarations: []\n"),
        }
        Some(s)
    }
}

/// Whether a model node has a non-empty `decorators` array: TS's
/// `processModels` only extracts a model's own decorators then.
fn has_decorators(model: &Value) -> bool {
    model
        .get("decorators")
        .and_then(Value::as_array)
        .is_some_and(|d| !d.is_empty())
}

/// `DecoratorExtractor.constructDCSDictionary` (`src/decoratorextractor.ts`),
/// for `node`'s own `decorators` array, when it has one.
fn construct_dcs_dictionary<'a>(
    extraction_dictionary: &mut ExtractionDictionary<'a>,
    node: &'a Value,
    namespace: &'a str,
    target: ExtractedDecorators<'a>,
) {
    let Some(decorators) = node.get("decorators").and_then(Value::as_array) else {
        return;
    };
    let entry = ExtractedDecorators {
        decorators,
        ..target
    };
    match extraction_dictionary
        .iter_mut()
        .find(|(k, _)| *k == namespace)
    {
        Some((_, entries)) => entries.push(entry),
        None => extraction_dictionary.push((namespace, vec![entry])),
    }
}

/// The reading half of `DecoratorExtractor.processModels` (with
/// `processDeclarations`, `processMapDeclaration` and `processProperties`,
/// `src/decoratorextractor.ts`): every decorated node, in TS's walk order,
/// recorded by borrow into `extraction_dictionary`. Nothing is cloned; the
/// names are the AST's own strings (`""` for a missing one, as TS's
/// `obj.declaration || ''` reads).
fn collect_models<'a>(extraction_dictionary: &mut ExtractionDictionary<'a>, models: &'a [Value]) {
    for model in models {
        let namespace = model
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if has_decorators(model) {
            construct_dcs_dictionary(
                extraction_dictionary,
                model,
                namespace,
                ExtractedDecorators::default(),
            );
        }
        let Some(Value::Array(declarations)) = model.get("declarations") else {
            continue;
        };
        for decl in declarations {
            let declaration = decl.get("name").and_then(Value::as_str).unwrap_or_default();
            let at_declaration = ExtractedDecorators {
                declaration,
                ..Default::default()
            };
            construct_dcs_dictionary(extraction_dictionary, decl, namespace, at_declaration);
            if decl.get("$class").and_then(Value::as_str) == Some(MAP_DECLARATION_CLASS) {
                for (element, map_element) in [("key", "KEY"), ("value", "VALUE")] {
                    if let Some(node) = decl.get(element) {
                        construct_dcs_dictionary(
                            extraction_dictionary,
                            node,
                            namespace,
                            ExtractedDecorators {
                                map_element,
                                ..at_declaration
                            },
                        );
                    }
                }
            }
            if let Some(Value::Array(properties)) = decl.get("properties") {
                for property in properties {
                    let property_name = property
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    construct_dcs_dictionary(
                        extraction_dictionary,
                        property,
                        namespace,
                        ExtractedDecorators {
                            property: property_name,
                            ..at_declaration
                        },
                    );
                }
            }
        }
    }
}

/// `Object.keys(x).length > 0 ? x[key] : (x[key] = {})`, for the nested
/// `vocabObject.namespace`/`.declarations`/`.propertyVocabs` objects
/// `parseVocabularies` builds up incrementally (`vocabObject.namespace =
/// vocabObject.namespace || {}`, `src/decoratorextractor.ts`).
fn ensure_object<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
    if !matches!(value.get(key), Some(Value::Object(_)))
        && let Some(map) = value.as_object_mut()
    {
        map.insert(key.to_string(), Value::Object(Map::new()));
    }
    value
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .expect("just ensured an object at key")
}

#[cfg(test)]
mod tests {
    use super::super::DECORATOR_STRING_TYPE;
    use super::*;
    use serde_json::json;

    fn decorator(name: &str, value: &str) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Decorator",
            "name": name,
            "arguments": [
                { "$class": DECORATOR_STRING_TYPE, "value": value }
            ]
        })
    }

    fn sample_models() -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "test@1.0.0",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "decorators": [decorator("Term", "Person"), decorator("Custom", "hi")],
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "name",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [decorator("Term", "Name")]
                    }]
                }]
            }]
        })
    }

    #[test]
    fn extracts_a_vocabulary_and_a_non_vocabulary_command_set() {
        let extractor =
            DecoratorExtractor::new(true, "en", "0.4.0", sample_models(), Action::ExtractAll);
        let result = extractor.extract().expect("extraction succeeds");

        assert_eq!(result.decorator_command_set.len(), 1);
        let dcs = &result.decorator_command_set[0];
        assert_eq!(dcs["commands"].as_array().unwrap().len(), 1);
        assert_eq!(dcs["commands"][0]["decorator"]["name"], "Custom");

        assert_eq!(result.vocabularies.len(), 1);
        let vocab = &result.vocabularies[0];
        assert!(vocab.contains("locale: en\n"));
        assert!(vocab.contains("namespace: test@1.0.0\n"));
        assert!(vocab.contains("  - Person: Person\n"));
        assert!(vocab.contains("      - name: Name\n"));

        // removeDecoratorsFromModel + EXTRACT_ALL strips every decorator.
        let person =
            &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
        assert!(person.get("decorators").is_none());
    }

    #[test]
    fn extract_vocab_only_leaves_non_vocab_decorators_in_place() {
        let extractor =
            DecoratorExtractor::new(true, "en", "0.4.0", sample_models(), Action::ExtractVocab);
        let result = extractor.extract().expect("extraction succeeds");
        assert!(result.decorator_command_set.is_empty());
        assert_eq!(result.vocabularies.len(), 1);

        let person =
            &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
        let names: Vec<&str> = person["decorators"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Custom"]);
    }

    #[test]
    fn without_remove_the_result_models_are_the_source_models() {
        let source = sample_models();
        let extractor =
            DecoratorExtractor::new(false, "en", "0.4.0", source.clone(), Action::ExtractAll);
        let result = extractor.extract().expect("extraction succeeds");
        assert_eq!(result.decorator_command_set.len(), 1);
        assert_eq!(result.vocabularies.len(), 1);
        assert_eq!(
            result.model_manager.model_file("test@1.0.0").unwrap().ast(),
            &source["models"][0]
        );
    }

    #[test]
    fn a_model_without_declarations_is_given_an_empty_array() {
        let models = json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "test@1.0.0",
                "decorators": [decorator("Term", "Test")]
            }]
        });
        let extractor = DecoratorExtractor::new(true, "en", "0.4.0", models, Action::ExtractAll);
        let result = extractor.extract().expect("extraction succeeds");
        let ast = result.model_manager.model_file("test@1.0.0").unwrap().ast();
        assert!(ast.get("decorators").is_none());
        assert_eq!(ast["declarations"], json!([]));
        assert_eq!(
            result.vocabularies,
            vec!["locale: en\nnamespace: test@1.0.0\nterm: Test\ndeclarations: []\n"]
        );
    }

    #[test]
    fn map_keys_and_values_are_extracted_and_stripped() {
        let models = json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "test@1.0.0",
                "declarations": [{
                    "$class": MAP_DECLARATION_CLASS,
                    "name": "Dictionary",
                    "key": {
                        "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                        "decorators": [decorator("Term", "Word"), decorator("Custom", "k")]
                    },
                    "value": {
                        "$class": "concerto.metamodel@1.0.0.StringMapValueType",
                        "decorators": [decorator("Custom", "v")]
                    }
                }]
            }]
        });
        let extractor =
            DecoratorExtractor::new(true, "en", "0.4.0", models, Action::ExtractNonVocab);
        let result = extractor.extract().expect("extraction succeeds");
        let commands = result.decorator_command_set[0]["commands"]
            .as_array()
            .unwrap();
        let targets: Vec<&Value> = commands
            .iter()
            .map(|c| &c["target"]["mapElement"])
            .collect();
        assert_eq!(targets, vec!["KEY", "VALUE"]);
        assert!(result.vocabularies.is_empty());

        // EXTRACT_NON_VOCAB with removal keeps only the vocabulary decorators.
        let map = &result.model_manager.model_file("test@1.0.0").unwrap().ast()["declarations"][0];
        let key_names: Vec<&str> = map["key"]["decorators"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(key_names, vec!["Term"]);
        assert_eq!(map["value"]["decorators"], json!([]));
    }

    /// P5-57: the direct encoding and the `Value` route agree byte for byte
    /// (command-set text, vocabularies, result models), or fail with the
    /// same error, for every action and both `removeDecoratorsFromModel`
    /// settings.
    fn assert_routes_agree(models: &Value, expect_ok: bool) {
        for action in [
            Action::ExtractAll,
            Action::ExtractVocab,
            Action::ExtractNonVocab,
        ] {
            for remove in [false, true] {
                let value_route =
                    DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action)
                        .extract();
                let direct = DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action)
                    .extract_encoded();
                match (value_route, direct) {
                    (Ok(v), Ok(d)) => {
                        assert_eq!(
                            d.decorator_command_set,
                            serde_json::to_string(&v.decorator_command_set).unwrap(),
                            "{action:?} remove={remove}"
                        );
                        assert_eq!(d.vocabularies, v.vocabularies, "{action:?} remove={remove}");
                        let asts = |mm: &ModelManager| {
                            mm.model_files()
                                .map(|f| f.ast().clone())
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(asts(&d.model_manager), asts(&v.model_manager));
                    }
                    (Err(v), Err(d)) => {
                        assert!(!expect_ok, "{action:?} remove={remove}: {v}");
                        assert_eq!(d, v, "{action:?} remove={remove}");
                    }
                    (v, d) => panic!(
                        "{action:?} remove={remove}: the routes disagree: value ok {}, direct ok {}",
                        v.is_ok(),
                        d.is_ok()
                    ),
                }
                assert_memo_route_agrees(models, action, remove);
            }
        }
    }

    /// P5-56 (T2, F-A2): `extract_encoded_keeping_source` is
    /// `extract_encoded` (same result, same error), and `encode_source` over
    /// the kept models gives the same command sets and vocabularies (or the
    /// same error) as `extract_encoded` with any locale and either
    /// `removeDecoratorsFromModel`, every time it is called.
    fn assert_memo_route_agrees(models: &Value, action: Action, remove: bool) {
        let direct = DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action)
            .extract_encoded();
        let keeping = DecoratorExtractor::new(remove, "fr", "0.4.0", models.clone(), action)
            .extract_encoded_keeping_source();
        let (kept_result, source) = match (direct, keeping) {
            (Ok(d), Ok(k)) => (Some((d, k.0)), k.1),
            (Err(d), Err(k)) => {
                assert_eq!(k, d, "{action:?} remove={remove}");
                (
                    None,
                    models["models"].as_array().cloned().unwrap_or_default(),
                )
            }
            (d, k) => panic!(
                "{action:?} remove={remove}: direct ok {}, keeping ok {}",
                d.is_ok(),
                k.is_ok()
            ),
        };
        if let Some((d, k)) = &kept_result {
            assert_eq!(k.decorator_command_set, d.decorator_command_set);
            assert_eq!(k.vocabularies, d.vocabularies);
            let asts = |mm: &ModelManager| {
                mm.model_files()
                    .map(|f| f.ast().clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(asts(&k.model_manager), asts(&d.model_manager));
            assert_eq!(&source, models["models"].as_array().unwrap());
        }
        for (other_remove, locale) in [(remove, "fr"), (!remove, "de"), (remove, "fr")] {
            let encoded =
                DecoratorExtractor::new(other_remove, locale, "0.4.0", Value::Null, action)
                    .encode_source(&source);
            let fresh =
                DecoratorExtractor::new(other_remove, locale, "0.4.0", models.clone(), action)
                    .extract_encoded();
            match (encoded, fresh) {
                (Ok(e), Ok(f)) => {
                    assert_eq!(e.0, f.decorator_command_set, "{action:?} {locale}");
                    assert_eq!(e.1, f.vocabularies, "{action:?} {locale}");
                }
                // A result-model error comes first in `extract_encoded`; the
                // memo is only kept after a call that did not fail, so only
                // the transform's own errors can reach `encode_source`.
                (Err(e), Err(f)) => assert_eq!(e, f, "{action:?} {locale}"),
                (Ok(_), Err(_)) if kept_result.is_none() => {}
                (e, f) => panic!(
                    "{action:?} {locale}: encode_source ok {}, extract_encoded ok {}",
                    e.is_ok(),
                    f.is_ok()
                ),
            }
        }
    }

    #[test]
    fn the_direct_encoding_matches_the_value_route() {
        assert_routes_agree(&sample_models(), true);

        let dec = |name: Value, args: Value| {
            let mut d =
                json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "arguments": args });
            if !name.is_null() {
                d["name"] = name;
            }
            d
        };
        let s = |v: &str| json!([{ "$class": DECORATOR_STRING_TYPE, "value": v }]);
        let models = json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.p557@1.2.3",
                "decorators": [
                    dec(json!("Term_description"), s("ns \"quoted\"")),
                    dec(json!("Term"), s("Namespace")),
                    dec(json!("Term_term"), s("Replaced")),
                    dec(json!("Term_"), s("empty key")),
                    dec(json!("Meta"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1 }])),
                ],
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "decorators": [
                        dec(json!("Term_plural"), s("People")),
                        dec(json!("Flag"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": true }])),
                        dec(json!("Ref"), json!([{
                            "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person", "namespace": "org.p557@1.2.3" },
                            "isArray": true
                        }])),
                        json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "NoArgs" }),
                        dec(json!("Odd"), json!([
                            { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 2.5e-7 },
                            { "$class": DECORATOR_STRING_TYPE, "value": "tab\t \"q\" \u{e9} \u{1F600} </" }
                        ])),
                    ],
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "name",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [
                            dec(json!("Term_description"), s("The name")),
                            dec(json!("Term"), s("Name")),
                            dec(json!("Term_propertyVocabs"), s("kept")),
                            dec(json!("Custom"), json!([])),
                        ]
                    }, {
                        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                        "name": "age",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [dec(json!("Term_unit"), json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 10 }]))]
                    }]
                }, {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Overwritten",
                    "isAbstract": false,
                    "decorators": [dec(json!("Term_propertyVocabs"), s("gone"))],
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "before",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [dec(json!("Term"), s("Before"))]
                    }]
                }, {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Plain",
                    "isAbstract": false,
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "only",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [dec(json!("Term_x"), s("x"))]
                    }]
                }, {
                    "$class": MAP_DECLARATION_CLASS,
                    "name": "Dictionary",
                    "key": {
                        "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                        "decorators": [dec(json!("Term"), s("Word")), dec(json!("Custom"), s("k"))]
                    },
                    "value": {
                        "$class": "concerto.metamodel@1.0.0.StringMapValueType",
                        "decorators": [dec(json!("Term_meaning"), s("Meaning"))]
                    }
                }]
            }, {
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.other@0.0.1-rc.1",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Thing",
                    "isAbstract": false,
                    "decorators": [dec(json!("Custom"), s("thing"))],
                    "properties": []
                }]
            }]
        });
        assert_routes_agree(&models, true);

        // Each reserved-key error, from both routes.
        for (target, name) in [
            ("model", "Term_locale"),
            ("declaration", "Term_properties"),
            ("declaration", "Term_Person"),
            ("property", "Term_name"),
        ] {
            let mut bad = sample_models();
            let node = match target {
                "model" => &mut bad["models"][0],
                "declaration" => &mut bad["models"][0]["declarations"][0],
                _ => &mut bad["models"][0]["declarations"][0]["properties"][0],
            };
            node["decorators"] = json!([decorator("Custom", "first"), decorator(name, "oops")]);
            assert_routes_agree(&bad, false);
        }
    }

    /// P5-98 (C-11): a `Term_*` decorator's number argument is written in
    /// the vocabulary YAML as JS `String(value)` writes it — TS 5.0.0's
    /// `extractDecorators` gives `unit: 0.000001` and `max: 1e+21`, where
    /// serde_json's own `Display` gave `1e-6` and `1e21`.
    #[test]
    fn a_vocabulary_number_argument_is_written_as_js_string_does() {
        let number = |v: Value| json!([{ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": v }]);
        let dec = |name: &str, args: Value| json!({ "$class": "concerto.metamodel@1.0.0.Decorator", "name": name, "arguments": args });
        let models = json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "test@1.0.0",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "P",
                    "isAbstract": false,
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                        "name": "age",
                        "isArray": false,
                        "isOptional": false,
                        "decorators": [
                            dec("Term_unit", number(json!(0.000001))),
                            dec("Term_max", number(json!(1e21))),
                        ]
                    }]
                }]
            }]
        });
        let result =
            DecoratorExtractor::new(false, "en", "0.4.0", models.clone(), Action::ExtractAll)
                .extract()
                .unwrap();
        assert_eq!(
            result.vocabularies,
            [
                "locale: en\nnamespace: test@1.0.0\ndeclarations:\n  - P: P\n    properties:\n      - age: age\n        unit: 0.000001\n        max: 1e+21\n"
            ]
        );
        assert_routes_agree(&models, true);
    }

    #[test]
    fn a_term_extension_key_matching_the_declaration_name_is_rejected() {
        let models = json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "test@1.0.0",
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "decorators": [decorator("Term_Person", "oops")],
                    "properties": []
                }]
            }]
        });
        let extractor = DecoratorExtractor::new(false, "en", "0.4.0", models, Action::ExtractAll);
        let err = match extractor.extract() {
            Ok(_) => panic!("expected extraction to reject the reserved vocabulary key"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Invalid vocabulary key"));
    }
}
