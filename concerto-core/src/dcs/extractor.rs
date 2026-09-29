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

use crate::error::{ContractError, ErrorKind, Result};
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

use super::{MAP_DECLARATION_CLASS, META_MODEL_NAMESPACE, quote_string_value};

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

/// The `$class` strings every command a [`DecoratorExtractor`] builds
/// repeats, formatted once per extraction rather than once per decorator.
struct CommandClasses {
    command: String,
    target: String,
    decorator: String,
    type_reference: String,
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
                s.push_str(&format!("term: {}\n", display_value(term)));
            }
            for (key, value) in ns_obj {
                if key != "term" {
                    s.push_str(&format!("{key}: {}\n", display_value(value)));
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
                        display_value(&decl_obj["term"])
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
                            display_value(&decl_obj[key.as_str()])
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
                            .map(display_value)
                            .unwrap_or_else(|| prop.clone());
                        s.push_str(&format!("      - {prop}: {term_val}\n"));
                        for (key, value) in prop_obj {
                            if key != "term" {
                                s.push_str(&format!("        {key}: {}\n", display_value(value)));
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
            Value::String(classes.decorator.clone()),
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
                    let is_type_reference = class.as_str() == Some(classes.type_reference.as_str());
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
        let arg0 = dcs.get("arguments").and_then(|a| a.get(0));
        let arg_value = arg0
            .and_then(|a| a.get("value"))
            .map(display_value)
            .unwrap_or_default();
        let arg_class = arg0.and_then(|a| a.get("$class")).and_then(Value::as_str);
        let quoted = quote_string_value(&arg_value, arg_class);

        if vocab_target.declaration.is_empty() {
            let ns = ensure_object(vocab_object, "namespace");
            if dcs_name == "Term" {
                ns.insert("term".to_string(), Value::String(quoted));
            } else {
                let extension_key = dcs_name.strip_prefix("Term_").unwrap_or(dcs_name);
                if matches!(extension_key, "namespace" | "locale" | "declarations") {
                    return Err(ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!("Invalid vocabulary key: {extension_key}. The key should not be one of the reserved keys: namespace, locale, declarations"),
                        None,
                    )
                    .into());
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
                    return Err(ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!("Invalid vocabulary key: \"{extension_key}\". The key should not be the name of the current property."),
                        None,
                    )
                    .into());
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
                    return Err(ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!("Invalid vocabulary key: \"{extension_key}\". The key should not be the name of the current property."),
                        None,
                    )
                    .into());
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
                    return Err(ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!("Invalid vocabulary key: \"{extension_key}\". The key cannot be a reserved word such as \"properties\" or the name of the current declaration."),
                        None,
                    )
                    .into());
                }
                decl_obj.insert(extension_key.to_string(), Value::String(quoted));
            }
        }
        Ok(())
    }

    /// `DecoratorExtractor.transformDecoratorsAndVocabularies`
    /// (`src/decoratorextractor.ts`), over the borrowed dictionary
    /// [`collect_models`] built.
    fn transform_decorators_and_vocabularies(
        &self,
        extraction_dictionary: &ExtractionDictionary<'_>,
    ) -> Result<(Vec<Value>, Vec<String>)> {
        let version = &self.dcs_version;
        let classes = CommandClasses {
            command: format!("org.accordproject.decoratorcommands@{version}.Command"),
            target: format!("org.accordproject.decoratorcommands@{version}.CommandTarget"),
            decorator: format!("{META_MODEL_NAMESPACE}.Decorator"),
            type_reference: format!("{META_MODEL_NAMESPACE}.DecoratorTypeReference"),
        };
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
    pub fn extract(mut self) -> Result<ExtractResult> {
        let mut models = match self.updated_model_ast.get_mut("models").map(std::mem::take) {
            Some(Value::Array(models)) => models,
            _ => Vec::new(),
        };
        let transformed = {
            let mut extraction_dictionary = ExtractionDictionary::new();
            collect_models(&mut extraction_dictionary, &models);
            self.transform_decorators_and_vocabularies(&extraction_dictionary)
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

        let (decorator_command_set, vocabularies) = transformed?;
        Ok(ExtractResult {
            model_manager,
            decorator_command_set,
            vocabularies,
        })
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

/// The plain (unquoted) text a decorator argument's JSON value reads as in
/// the vocabulary YAML this module hand-builds line by line — a string as
/// itself (JS template-literal interpolation, `String(value)`), any other
/// JSON scalar via its JSON text.
fn display_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
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
