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

use serde_json::Value;

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
/// (`src/decoratormanager.ts`'s JSDoc typedef), with its command sets
/// already encoded as JSON text (P5-57, T3, accordproject/concerto-rust#378).
pub struct ExtractResult {
    /// A model manager over the (possibly decorator-stripped) models.
    pub model_manager: ModelManager,
    /// The extracted, non-vocabulary decorators: the JSON text of the array
    /// of `DecoratorCommandSet` objects (one per namespace that had any).
    pub decorator_command_set: String,
    /// The extracted vocabulary (`Term`/`Term_*`) decorators, as vocabulary
    /// YAML strings (one per namespace that had any).
    pub vocabularies: Vec<String>,
    /// A copy of the source models the walk read, taken before it, when
    /// the extraction was asked to keep them (P5-56, T2, F-A2,
    /// accordproject/concerto-rust#377): [`DecoratorExtractor::encode_source`]
    /// over them gives exactly this result's command sets and vocabularies.
    pub source_models: Option<Vec<Value>>,
}

/// The `$class` strings every command a [`DecoratorExtractor`] builds
/// repeats, formatted once per extraction rather than once per decorator.
struct CommandClasses {
    command: String,
    target: String,
    decorator: &'static str,
    type_reference: &'static str,
}

/// `DecoratorExtractor` (`src/decoratorextractor.ts`): its configuration
/// only. The models it walks are each call's input
/// ([`Self::extract`], [`Self::encode_source`]), not part of it (P5-104,
/// C-8), so an extractor borrows its locale rather than owning a copy.
pub(crate) struct DecoratorExtractor<'a> {
    remove_decorators_from_model: bool,
    locale: &'a str,
    dcs_version: &'a str,
    action: Action,
}

impl<'a> DecoratorExtractor<'a> {
    /// `new DecoratorExtractor(...)` (`src/decoratorextractor.ts`), without
    /// its source models, which [`Self::extract`] takes.
    pub(crate) fn new(
        remove_decorators_from_model: bool,
        locale: &'a str,
        dcs_version: &'a str,
        action: Action,
    ) -> Self {
        Self {
            remove_decorators_from_model,
            locale,
            dcs_version,
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

    /// The `$class` strings of this extraction's commands.
    fn command_classes(&self) -> CommandClasses {
        let version = self.dcs_version;
        CommandClasses {
            command: format!("org.accordproject.decoratorcommands@{version}.Command"),
            target: format!("org.accordproject.decoratorcommands@{version}.CommandTarget"),
            decorator: metamodel_class!("Decorator"),
            type_reference: metamodel_class!("DecoratorTypeReference"),
        }
    }

    /// `DecoratorExtractor.transformDecoratorsAndVocabularies`
    /// (`src/decoratorextractor.ts`), over the borrowed dictionary
    /// [`collect_models`] built, without any intermediate [`Value`] (P5-57,
    /// T3, accordproject/concerto-rust#378): the command sets as the JSON
    /// text of the `DecoratorCommandSet` array, serialised through borrowed
    /// views of the AST nodes ([`CommandSetView`]), and the vocabularies
    /// from a borrowed tree ([`VocabTree`]) in place of TS's `vocabObject`.
    /// P5-103 (C-5) deleted the `Value` route this replaced.
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
                && let Some(yaml) = vocab.to_yaml(self.locale, namespace)
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

    /// The command sets (as JSON text) and vocabularies that
    /// [`Self::extract`] would return for `models` (P5-56): the same
    /// walk and the same transform, with the same first error, but no
    /// result manager. `models` are the source models of an earlier
    /// extraction ([`ExtractResult::source_models`]); this
    /// extractor's own source AST is not read. The command sets and
    /// vocabularies are read before any decorator is stripped, so
    /// `removeDecoratorsFromModel` does not change them.
    pub(crate) fn encode_source(&self, models: &[Value]) -> Result<(String, Vec<String>)> {
        let mut extraction_dictionary = ExtractionDictionary::new();
        collect_models(&mut extraction_dictionary, models);
        self.encode_decorators_and_vocabularies(&extraction_dictionary)
    }

    /// `DecoratorExtractor.extract` (`src/decoratorextractor.ts`), with the
    /// command sets and vocabularies encoded directly from the borrowed AST
    /// nodes ([`Self::encode_decorators_and_vocabularies`]), and a copy of
    /// the source models kept in the result when `keep_source` is set.
    ///
    /// P5-40 (F-B): the models are walked twice rather than once. The first
    /// walk ([`collect_models`]) only borrows them, recording where each
    /// `decorators` array is; the command sets and vocabularies are built
    /// from those borrows before the second walk
    /// ([`Self::process_models`]) strips the decorators in place, and the
    /// models are then moved, not copied, into the result manager. Errors
    /// keep TS's order: a load or validation failure of the result models
    /// is thrown ahead of a vocabulary-key error from the transform.
    ///
    /// `models` are the source models, the `models` of an `IModels`
    /// envelope (TS's `sourceModelAst.models`).
    pub(crate) fn extract(
        &self,
        mut models: Vec<Value>,
        keep_source: bool,
    ) -> Result<ExtractResult> {
        let source_models = keep_source.then(|| models.clone());
        let transformed = {
            let mut extraction_dictionary = ExtractionDictionary::new();
            collect_models(&mut extraction_dictionary, &models);
            self.encode_decorators_and_vocabularies(&extraction_dictionary)
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
            source_models,
        })
    }
}

/// `null`, for a node's missing field, borrowed.
static NULL: Value = Value::Null;

/// P5-57: one `DecoratorCommandSet`, as TS's
/// `transformNonVocabularyDecorators` builds it (same keys, same order),
/// serialised from borrows.
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

/// P5-57: one `UPSERT` command, as TS's `parseNonVocabularyDecorators`
/// builds it (with `constructTarget`'s target), serialised from the
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

/// [`CommandView`]'s `target`: TS's `constructTarget`.
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
    /// `DecoratorExtractor.parseVocabularies` (`src/decoratorextractor.ts`),
    /// into this tree.
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

    /// `DecoratorExtractor.transformVocabularyDecorators`: the YAML of
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

#[cfg(test)]
#[path = "tests/extractor.rs"]
mod tests;
