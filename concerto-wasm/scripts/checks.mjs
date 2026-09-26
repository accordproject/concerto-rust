// The smoke checks, shared by the Node and headless-Chromium smokes. They run
// against a loaded engine module (pkg/concerto-engine.cjs or .mjs) and return
// one {name, ok, detail?} row per check; a smoke fails if any row is not ok.
//
// They cover the handle API (one ModelManagerHandle per manager, dense u32
// handles, JSON snapshots, generation()), the error mapping through the
// registered factory, and one of the P0-04b trial bindings.

const MM = 'concerto.metamodel@1.0.0';

const MODEL = {
  $class: `${MM}.Model`,
  namespace: 'org.example@1.0.0',
  imports: [],
  declarations: [
    {
      $class: `${MM}.ConceptDeclaration`, name: 'Person', isAbstract: false,
      properties: [
        { $class: `${MM}.StringProperty`, name: 'name', isArray: false, isOptional: false },
        { $class: `${MM}.IntegerProperty`, name: 'age', isArray: false, isOptional: true },
      ],
    },
    {
      $class: `${MM}.ConceptDeclaration`, name: 'Employee', isAbstract: false,
      superType: { $class: `${MM}.TypeIdentifier`, name: 'Person' },
      properties: [
        { $class: `${MM}.DoubleProperty`, name: 'salary', isArray: false, isOptional: false },
      ],
    },
    {
      $class: `${MM}.EnumDeclaration`, name: 'Color',
      properties: [{ $class: `${MM}.EnumProperty`, name: 'RED' }],
    },
  ],
};

const BROKEN = {
  $class: `${MM}.Model`,
  namespace: 'org.broken@1.0.0',
  imports: [],
  declarations: [
    {
      $class: `${MM}.ConceptDeclaration`, name: 'Orphan', isAbstract: false,
      superType: { $class: `${MM}.TypeIdentifier`, name: 'Ghost' },
      properties: [],
    },
  ],
};

/** The error the smokes' factory builds: it keeps the payload it was given. */
class EngineError extends Error {
  constructor(payload) {
    super(payload.message);
    this.name = `EngineError(${payload.kind})`;
    this.payload = payload;
  }
}

/** Runs `fn`, expecting it to throw; returns what it threw. */
function thrown(fn) {
  try {
    fn();
  } catch (e) {
    return e;
  }
  throw new Error('expected an exception');
}

export function runChecks(engine) {
  const rows = [];
  const check = (name, fn) => {
    try {
      const detail = fn();
      rows.push({ name, ok: true, ...(detail === undefined ? {} : { detail }) });
    } catch (e) {
      rows.push({ name, ok: false, detail: `${e.name}: ${e.message}` });
    }
  };
  const assert = (cond, message) => {
    if (!cond) throw new Error(message);
  };

  engine.setHost((payload) => new EngineError(payload), (version) => ({ version }));

  const mm = new engine.ModelManagerHandle();
  let file;
  let person;

  check('a new manager holds the system model', () => {
    // P1-07b (#101): a fresh manager preloads `concerto.decorator@1.0.0`
    // before `concerto@1.0.0`, matching TS's
    // `addDecoratorModel(); addRootModel();`, so it starts with two model
    // files, decorator first.
    const ids = [...mm.modelFileIds()];
    assert(ids.length === 2 && ids[0] === 0 && ids[1] === 1, `modelFileIds ${ids}`);
    assert(mm.modelFileId('concerto.decorator@1.0.0') === 0, 'concerto.decorator@1.0.0 is file 0');
    assert(mm.modelFileId('concerto@1.0.0') === 1, 'concerto@1.0.0 is file 1');
    assert(typeof mm.declarationId('concerto@1.0.0.Concept') === 'number', 'Concept has a handle');
    assert(typeof mm.declarationId('concerto.decorator@1.0.0.Decorator') === 'number', 'Decorator has a handle');
    return { generation: mm.generation() };
  });

  check('addModel returns the model file handle and bumps the generation', () => {
    const before = mm.generation();
    file = mm.addModel(JSON.stringify(MODEL), 'example.cto');
    assert(typeof file === 'number', `handle ${file}`);
    assert(mm.modelFileId('org.example@1.0.0') === file, 'modelFileId agrees');
    assert(mm.generation() === before + 1, `generation ${before} -> ${mm.generation()}`);
    return { file, generation: mm.generation() };
  });

  check('declaration handles and snapshots', () => {
    const ids = [...mm.declarationIds(file)];
    assert(ids.length === 3, `declarationIds ${ids}`);
    person = mm.declarationId('org.example@1.0.0.Person');
    assert(person === ids[0], `Person is ${person}, first is ${ids[0]}`);
    assert(mm.modelFileOf(person) === file, 'modelFileOf');
    const snap = JSON.parse(mm.declarationSnapshot(person));
    assert(snap.name === 'Person', `name ${snap.name}`);
    assert(snap.fullyQualifiedName === 'org.example@1.0.0.Person', `fqn ${snap.fullyQualifiedName}`);
    assert(snap.modelFile === file, 'snapshot modelFile');
    assert(JSON.stringify(snap.ast) === JSON.stringify(MODEL.declarations[0]), 'ast is the loaded node');
    assert(mm.declarationId('org.example@1.0.0.Nope') === undefined, 'unknown fqn is undefined');
  });

  check('property handles and snapshots', () => {
    const props = [...mm.propertyIds(person)];
    assert(props.length === 2, `propertyIds ${props}`);
    const names = props.map((p) => JSON.parse(mm.propertySnapshot(p)).name);
    assert(names.join() === 'name,age', `names ${names}`);
    for (const p of props) assert(mm.parentOf(p) === person, 'parentOf');
    const age = JSON.parse(mm.propertySnapshot(props[1]));
    assert(age.declaration === person, 'snapshot declaration');
    assert(JSON.stringify(age.ast) === JSON.stringify(MODEL.declarations[0].properties[1]), 'ast is the loaded node');
    // P2-04 (#48): enum values get PropIds too, addressed the same way a
    // class declaration's fields are, one per EnumProperty.
    const color = mm.declarationId('org.example@1.0.0.Color');
    const colorProps = [...mm.propertyIds(color)];
    assert(colorProps.length === 1, `enum PropId count ${colorProps.length}`);
    assert(JSON.parse(mm.propertySnapshot(colorProps[0])).name === 'RED', 'enum value name');
  });

  check('model file snapshot', () => {
    const snap = JSON.parse(mm.modelFileSnapshot(file));
    assert(snap.namespace === 'org.example@1.0.0', `namespace ${snap.namespace}`);
    assert(snap.version === '1.0.0', `version ${snap.version}`);
    assert(snap.fileName === 'example.cto', `fileName ${snap.fileName}`);
    assert(snap.ast.declarations.length === 3, 'ast');
  });

  check('handles stay valid across a later load', () => {
    const before = mm.declarationSnapshot(person);
    mm.addModel(JSON.stringify({ ...MODEL, namespace: 'org.other@1.0.0' }));
    assert(mm.declarationSnapshot(person) === before, 'same snapshot for the same handle');
    assert(mm.declarationId('org.example@1.0.0.Person') === person, 'same handle');
  });

  check('validateModels accepts a valid model set', () => {
    mm.validateModels();
  });

  check('errors leave through the registered factory', () => {
    // TS `BaseModelManager._throwAlreadyExists` throws a plain `Error`, never
    // the `IllegalModelException` this port raised before (P2-08b review).
    const dup = thrown(() => mm.addModel(JSON.stringify(MODEL)));
    assert(dup instanceof EngineError, `duplicate namespace threw ${dup}`);
    assert(dup.payload.kind === 'Error', `kind ${dup.payload.kind}`);
    assert(dup.payload.code === 'basemodelmanager-throwalreadyexists', `code ${dup.payload.code}`);
    const bad = new engine.ModelManagerHandle();
    bad.addModel(JSON.stringify(BROKEN));
    const invalid = thrown(() => bad.validateModels());
    assert(invalid instanceof EngineError, `validateModels threw ${invalid}`);
    bad.free();
    const handle = thrown(() => mm.declarationSnapshot(1e6));
    assert(handle instanceof EngineError && handle.payload.kind === 'TypeNotFound', `unknown handle threw ${handle}`);
    const json = thrown(() => mm.addModel('{'));
    assert(json instanceof SyntaxError, `malformed JSON threw ${json}`);
    // The manager is still usable after each of them.
    assert(mm.declarationId('org.example@1.0.0.Person') === person, 'usable after errors');
    return {
      duplicate: `${dup.payload.kind}/${dup.payload.code}: ${dup.message}`,
      validate: `${invalid.payload.kind}/${invalid.payload.code}: ${invalid.message}`,
      handle: `${handle.payload.kind}: ${handle.message}`,
    };
  });

  check('a freed manager throws', () => {
    const doomed = new engine.ModelManagerHandle();
    doomed.free();
    thrown(() => doomed.generation());
  });

  check('the trial bindings are still exported', () => {
    assert(engine.modelUtilGetShortName('org.example@1.0.0.Person') === 'Person', 'getShortName');
  });

  // P4-10 (accordproject/concerto-rust#69): a JS double crosses the
  // Serializer bindings exactly. Without serde_json's `float_roundtrip`
  // these came back 1 ULP off (989.9951327998888, 477.9526988316292).
  check('doubles cross the serializer bindings exactly', () => {
    const samples = [989.9951327998887, 477.95269883162916, 0.1, 5e-324, 1.7976931348623157e308];
    let seed = 0x2545f491;
    for (let i = 0; i < 2000; i += 1) {
      seed = (seed * 1103515245 + 12345) >>> 0;
      samples.push((seed / 0x100000000) * 10 ** ((i % 40) - 20));
    }
    for (const n of samples) {
      const back = JSON.parse(engine.populatorConvertPrimitive('Double', JSON.stringify(n), 'null', '$'));
      assert(Object.is(back, n), `populatorConvertPrimitive(${n}) returned ${back}`);
      const out = JSON.parse(engine.generatorConvertPrimitive('Double', JSON.stringify(n), 'null'));
      assert(Object.is(out, n), `generatorConvertPrimitive(${n}) returned ${out}`);
    }
    const mmd = new engine.ModelManagerHandle();
    mmd.addModel(JSON.stringify(MODEL));
    const env = { newId: () => 'id', nowMs: () => 0 };
    const doc = { $class: 'org.example@1.0.0.Employee', name: 'n', salary: 989.9951327998887 };
    const built = JSON.parse(mmd.serializerFromJson(JSON.stringify(doc), 'null', env));
    assert(Object.is(built.fields.salary, 989.9951327998887), `serializerFromJson salary ${built.fields.salary}`);
    const json = JSON.parse(mmd.serializerToJson(JSON.stringify(built), 'null'));
    assert(Object.is(json.salary, 989.9951327998887), `serializerToJson salary ${json.salary}`);
    mmd.free();
    return { samples: samples.length };
  });

  check('init() resolves on a loaded module', () => {
    assert(typeof engine.init === 'function', 'init is exported');
  });

  // P4-08c: the ModelFile bindings, keyed by the same model file handle.
  check('modelFile getters', () => {
    assert(mm.modelFileGetVersion(file) === '1.0.0', 'getVersion');
    assert(mm.modelFileIsSystemModelFile(file) === false, 'isSystemModelFile (user file)');
    const concertoFile = mm.modelFileId('concerto@1.0.0');
    assert(mm.modelFileIsSystemModelFile(concertoFile) === true, 'isSystemModelFile (system file)');
    assert(mm.modelFileGetVersion(concertoFile) === '1.0.0', 'getVersion (system file, versioned namespace)');
    const imports = [...mm.modelFileGetImports(file)];
    assert(imports.includes('concerto@1.0.0.Concept'), `getImports ${imports}`);
    assert(mm.modelFileIsLocalType(file, 'Person') === true, 'isLocalType Person');
    assert(mm.modelFileIsLocalType(file, 'Nope') === false, 'isLocalType Nope');
  });

  check('modelFileValidate accepts the loaded model', () => {
    mm.modelFileValidate(file);
  });

  check('modelFileValidateDetached validates a not-yet-registered file', () => {
    mm.modelFileValidateDetached(JSON.stringify({ ...MODEL, namespace: 'org.detached@1.0.0' }), undefined, undefined);
    const invalid = thrown(() => mm.modelFileValidateDetached(JSON.stringify(BROKEN), undefined, undefined));
    assert(invalid instanceof EngineError, `detached validate threw ${invalid}`);
  });

  check('modelFileFromAst builds a detached snapshot', () => {
    const snap = JSON.parse(engine.modelFileFromAst(MODEL, undefined, 'inline.cto'));
    assert(snap.namespace === 'org.example@1.0.0', `namespace ${snap.namespace}`);
    assert(snap.version === '1.0.0', `version ${snap.version}`);
    assert(snap.fileName === 'inline.cto', `fileName ${snap.fileName}`);
    assert(snap.isSystemModelFile === false, 'isSystemModelFile');
    assert(snap.imports.includes('concerto@1.0.0.Concept'), `imports ${snap.imports}`);
    // An unversioned namespace (only ever legal for the bare `concerto`
    // system namespace) reports `version: null`, matching TS's `undefined`.
    const bareSnap = JSON.parse(engine.modelFileFromAst({ ...MODEL, namespace: 'concerto' }, undefined, undefined));
    assert(bareSnap.version === null, `bare-namespace version ${bareSnap.version}`);
    const bad = thrown(() => engine.modelFileFromAst(null, undefined, undefined));
    assert(bad instanceof EngineError && bad.payload.code === 'pre-port', `bad ast threw ${bad}`);
  });

  check('modelFileFilter keeps only the predicate\'s declarations', () => {
    const target = new engine.ModelManagerHandle();
    const kept = mm.modelFileFilter(file, (fqn) => fqn === 'org.example@1.0.0.Person', target);
    assert(typeof kept === 'number', `filter returned ${kept}`);
    const ids = [...target.declarationIds(kept)];
    assert(ids.length === 1, `filtered declarationIds ${ids}`);
    assert(JSON.parse(target.declarationSnapshot(ids[0])).name === 'Person', 'kept declaration');
    const other = new engine.ModelManagerHandle();
    const dropped = mm.modelFileFilter(file, () => false, other);
    assert(dropped === undefined, `filter-to-nothing returned ${dropped}`);
    const threw = thrown(() => mm.modelFileFilter(file, () => { throw new Error('nope'); }, other));
    assert(threw.message === 'nope', `predicate error propagated as ${threw}`);
    target.free();
    other.free();
    // The source manager (and the file's own handle within it) is unchanged.
    assert(mm.declarationId('org.example@1.0.0.Person') === person, 'source manager untouched');
  });

  check('modelFileFilter gives the predicate the imported declaration\'s own FQN', () => {
    // A real cross-file import: `org.example@1.0.0.Person` is not the
    // relevant namespace here — `Person` belongs to `org.other@1.0.0`
    // (already loaded above), and `ModelFile::filter` calls the predicate
    // on it while pruning this file's own `ImportType` (module doc on
    // `concerto_core::ModelFile::filter`). A predicate keyed by the
    // importED file's real FQN, not the importING file's namespace, must
    // see `org.other@1.0.0.Person` and keep the import.
    const importer = {
      $class: MM,
      namespace: 'org.importer@1.0.0',
      imports: [
        { $class: `${MM}.ImportType`, namespace: 'org.other@1.0.0', name: 'Person' },
      ],
      declarations: [
        {
          $class: `${MM}.ConceptDeclaration`, name: 'Holder', isAbstract: false,
          properties: [
            {
              $class: `${MM}.ObjectProperty`, name: 'owner', isArray: false, isOptional: false,
              type: { $class: `${MM}.TypeIdentifier`, namespace: 'org.other@1.0.0', name: 'Person' },
            },
          ],
        },
      ],
    };
    const importerFile = mm.addModel(JSON.stringify(importer), 'importer.cto');
    const seen = [];
    const target = new engine.ModelManagerHandle();
    const kept = mm.modelFileFilter(importerFile, (fqn) => {
      seen.push(fqn);
      return true;
    }, target);
    assert(typeof kept === 'number', `filter returned ${kept}`);
    assert(seen.includes('org.other@1.0.0.Person'), `predicate saw ${JSON.stringify(seen)}`);
    assert(!seen.includes('org.importer@1.0.0.Person'), `predicate wrongly saw ${JSON.stringify(seen)}`);
    const snap = JSON.parse(target.modelFileSnapshot(kept));
    assert(
      snap.ast.imports.some((imp) => imp.namespace === 'org.other@1.0.0' && imp.name === 'Person'),
      `import kept ${JSON.stringify(snap.ast.imports)}`,
    );
    target.free();
  });

  // P4-08e: `setDecoratorValidation`, the TS `options.decoratorValidation`
  // setter added on the same pattern as
  // `setDangerouslyAllowReservedSystemTypeNamesInUserModels` (P4-08a). A
  // decorator whose name is not a declared type ("Nope") is otherwise
  // silently allowed; only `missingDecorator: 'error'` turns it into a
  // validateModels() failure (validation.rs
  // `undeclared_decorator_is_reported_before_the_duplicate_scan_when_decorator_validation_is_enabled`).
  check('setDecoratorValidation gates undeclared decorators', () => {
    const withDecorator = {
      ...MODEL,
      namespace: 'org.decorated@1.0.0',
      declarations: [
        {
          $class: `${MM}.ConceptDeclaration`, name: 'Widget', isAbstract: false,
          decorators: [{ $class: `${MM}.Decorator`, name: 'Nope', arguments: [] }],
          properties: [],
        },
      ],
    };
    const off = new engine.ModelManagerHandle();
    off.addModel(JSON.stringify(withDecorator));
    off.validateModels();
    off.free();

    const on = new engine.ModelManagerHandle();
    on.setDecoratorValidation({ missingDecorator: 'error' });
    on.addModel(JSON.stringify(withDecorator));
    const err = thrown(() => on.validateModels());
    assert(err instanceof EngineError, `validateModels threw ${err}`);
    assert(/Undeclared type/.test(err.message), `message ${err.message}`);
    on.free();

    return { errorMessage: err.message };
  });

  // accordproject/concerto-rust#217 (T2a review finding 2, corrected): TS
  // assigns `this.superType = this.ast.superType.name` verbatim, so a
  // `superType` node with no `name` key at all ends up `undefined` there,
  // not `null` — and unlike `identified.name` (checked with a plain
  // truthiness test downstream), `superType` is checked with `!== null`, so
  // `undefined` is NOT read as "nothing to resolve": TS goes on to resolve a
  // super type named `undefined` (string concatenation coerces it to that
  // literal text) and fails with "Could not find super type undefined".
  // `classDeclarationProcess` must reproduce that distinction — this is
  // exactly the divergence the finding flagged: an earlier fix conflated
  // "no `name` key" with "explicit `name: null`" and silently accepted both
  // as "no super type", which is only correct for the latter.
  check('classDeclarationProcess tells a missing superType.name from an explicit null apart', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Foo',
      fqn: 'org.example@1.0.0.Foo',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    // `superType: {}` (present, but no `name` key): TS ends up with
    // `this.superType === undefined`, never the implicit 'Concept' default
    // (that only applies when `ast.superType` itself is absent) but also
    // never silently "no super type" — it is a real, unresolvable name,
    // reproduced here as the literal text "undefined".
    const noName = engine.classDeclarationProcess(mockDeclaration({ superType: {}, properties: [] }));
    assert(
      noName.superType === 'undefined',
      `superType:{} -> superType ${JSON.stringify(noName.superType)}`,
    );

    // `superType.name: null` explicitly: TS's `this.superType` is exactly
    // `null` here, which really does read as "no super type at all".
    const nullName = engine.classDeclarationProcess(
      mockDeclaration({ superType: { name: null }, properties: [] }),
    );
    assert(nullName.superType === null, `superType.name:null -> superType ${JSON.stringify(nullName.superType)}`);

    // `identified.name: null`, with a real `IdentifiedBy` $class: TS's
    // `this.idField = this.ast.identified.name` stays `null`, not the
    // literal text "null" a property lookup would then fail to find. Unlike
    // `superType`, `idField` is read with a plain truthiness check
    // downstream, so a missing `name` key would behave the same as an
    // explicit `null` here (both falsy) — no `undefined`/`null` distinction
    // to reproduce.
    const nullIdField = engine.classDeclarationProcess(
      mockDeclaration({
        identified: { $class: `${MM}.IdentifiedBy`, name: null },
        properties: [],
      }),
    );
    assert(nullIdField.idField === null, `identified.name:null -> idField ${JSON.stringify(nullIdField.idField)}`);

    // Sanity: an ordinary, present superType name still resolves normally.
    const named = engine.classDeclarationProcess(
      mockDeclaration({ superType: { name: 'Base' }, properties: [] }),
    );
    assert(named.superType === 'Base', `superType.name:"Base" -> superType ${JSON.stringify(named.superType)}`);

    return { noName, nullName, nullIdField, named };
  });

  // accordproject/concerto-rust#217 (T2a review finding 2, "only half
  // fixed"): the fix above only told nullish `superType`/`identified` names
  // apart; it left every other falsy shape — `0`, `false`, `""` — to fall
  // through to `js_string`, which stringifies them into truthy-looking text
  // (`"0"`, `"false"`) a property lookup then fails to find. TS's
  // `this.idField = this.ast.identified.name` is read everywhere downstream
  // with a plain truthiness check (`if (this.idField)`), so these three
  // must come back exactly like a nullish name: no id field at all.
  check('classDeclarationProcess treats a falsy, non-nullish identified.name as no id field', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Manufactured',
      fqn: 'org.example@1.0.0.Manufactured',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const results = {};
    for (const name of [0, false, '']) {
      const result = engine.classDeclarationProcess(
        mockDeclaration({
          identified: { $class: `${MM}.IdentifiedBy`, name },
          properties: [],
        }),
      );
      assert(
        result.idField === null,
        `identified.name:${JSON.stringify(name)} -> idField ${JSON.stringify(result.idField)}`,
      );
      results[JSON.stringify(name)] = result;
    }
    return results;
  });

  // accordproject/concerto-rust#217 (T2a review finding 2, the "only half
  // fixed" half, this time on `superType.name` — a re-review of the fix
  // above found it had wrongly folded this shape into "no super type"):
  // a falsy but non-nullish `superType.name` (`0`, `false`) is a
  // *different* shape from an explicit `null`
  // (`classDeclarationProcess tells a missing superType.name from an
  // explicit null apart`, above) — it must still come back as an explicit,
  // unresolvable super type, not `null`. TS's `this.superType =
  // this.ast.superType.name` is a plain assignment; `_resolveSuperType`'s
  // own `!this.superType` check does short-circuit on the falsiness without
  // throwing, but `validate`'s `this.getProperties()` does not go through
  // `_resolveSuperType` at all — it guards only on `this.superType !==
  // null` (true for both `0` and `false`), resolves directly, and throws
  // `Could not find super type 0`/`Could not find super type false`. Folding
  // these into `null` here would make Rust accept a model TS itself
  // rejects — the reverse of this issue's own ts=ok/rust=error shape.
  check('classDeclarationProcess treats a falsy, non-nullish superType.name as an unresolvable super type', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Employee',
      fqn: 'org.example@1.0.0.Employee',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const results = {};
    for (const [name, expected] of [[0, '0'], [false, 'false']]) {
      const result = engine.classDeclarationProcess(
        mockDeclaration({
          superType: { $class: `${MM}.TypeIdentifier`, name },
          properties: [],
        }),
      );
      assert(
        result.superType === expected,
        `superType.name:${JSON.stringify(name)} -> superType ${JSON.stringify(result.superType)}`,
      );
      results[JSON.stringify(name)] = result;
    }
    return results;
  });

  // accordproject/concerto-rust#217 (T2a review finding 2, the same "only
  // half fixed" gap, on `superType.name` instead): a *truthy* non-string
  // name (an array, a non-zero number, `true`, ...) is the other half TS
  // never stringifies — `this.superType` stays the raw value, and it is
  // only ever *used* as a string once resolution reaches
  // `ModelFile.getLocalType`'s `type.startsWith(this.getNamespace())`,
  // which throws `TypeError: type.startsWith is not a function` for
  // anything that isn't really a string. `js_string` (an earlier version of
  // this fix) coerced `["Vehicle"]` into the resolvable string `"Vehicle"`
  // instead, letting Rust load a model TS rejects.
  check('classDeclarationProcess reproduces the TypeError for a truthy non-string superType.name', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Car',
      fqn: 'org.example@1.0.0.Car',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const err = thrown(() =>
      engine.classDeclarationProcess(
        mockDeclaration({ superType: { name: ['Vehicle'] }, properties: [] }),
      ),
    );
    assert(err instanceof EngineError, `classDeclarationProcess threw ${err}`);
    assert(/type\.startsWith is not a function/.test(err.message), `message ${err.message}`);

    return { errorMessage: err.message };
  });

  // accordproject/concerto-rust#217 (T2a, adversarial review finding 1):
  // `collectionSizeValidatorNew`/`stringValidatorNew` used to decode
  // `sizeValidator`/`lengthValidator`/`validator` with `serde`'s strict,
  // typed `Deserialize` (a JSON number/string required for `minSize`/
  // `maxSize`/`minLength`/`maxLength`/`pattern`/`flags`), while TS's own
  // `CollectionSizeValidator`/`StringValidator` constructors
  // (collectionsizevalidator.ts, stringvalidator.ts) read every one of
  // these completely untyped: a fuzz-mutated bound that is present but not
  // a JSON number (or string, for `pattern`/`flags`) must coerce through
  // ECMAScript semantics, not fail the whole property's `process()`.
  check('collectionSizeValidatorNew/stringValidatorNew coerce a wrongly-typed bound instead of throwing', () => {
    const view = {};

    // `minSize: true` -> `Number(true)` is `1`, a valid (if unusual) bound;
    // TS never rejects it for being the wrong JSON type.
    const size = engine.collectionSizeValidatorNew(view, { minSize: true });
    assert(size.minSize === 1 && size.maxSize === null, `sizeValidator minSize:true -> ${JSON.stringify(size)}`);

    // `lengthValidator.minLength: true`, same coercion.
    const length = engine.stringValidatorNew(view, null, { minLength: true });
    assert(length.minLength === 1 && length.maxLength === null, `lengthValidator minLength:true -> ${JSON.stringify(length)}`);

    // `lengthValidator.maxLength` deleted (absent), `minLength` a real
    // number: TS's own `isNull(minLength) && isNull(maxLength)` is false
    // (minLength isn't null), so the "must be specified" check never
    // fires — this must not throw just because maxLength is absent.
    const oneBoundOnly = engine.stringValidatorNew(view, null, { minLength: 5 });
    assert(oneBoundOnly.minLength === 5, `lengthValidator minLength:5 (maxLength absent) -> ${JSON.stringify(oneBoundOnly)}`);

    // `validator.pattern: true` (a non-string pattern): `RegExp`'s own
    // `ToString` coercion (`String(true)` -> `"true"`, a valid pattern)
    // applies, not a decode failure.
    const regex = engine.stringValidatorNew(view, { pattern: true, flags: '' }, null);
    assert(regex.minLength === null && regex.maxLength === null, `validator pattern:true -> ${JSON.stringify(regex)}`);

    // A `lengthValidator` that is not even an object (a fuzz-mutated array,
    // matching a recorded cluster's minimised repro `lengthValidator: [10]`):
    // every key on it reads as absent, so this behaves as "no length
    // bounds at all" rather than throwing.
    const notAnObject = engine.stringValidatorNew(view, null, [10]);
    assert(notAnObject.minLength === null && notAnObject.maxLength === null, `lengthValidator:[10] -> ${JSON.stringify(notAnObject)}`);

    return { size, length, oneBoundOnly, regex, notAnObject };
  });

  // #218: two rust-mode view bindings, driven with minimal stand-in views.
  check('propertyProcess rejects a relationship with no type (DV-017)', () => {
    const modelFile = { getName: () => 'rel.cto' };
    for (const type of [undefined, null]) {
      const ast = { $class: `${MM}.RelationshipProperty`, name: 'dept', isArray: false, isOptional: false };
      if (type === null) ast.type = null;
      const err = thrown(() => engine.propertyProcess({ ast, getModelFile: () => modelFile }));
      assert(err instanceof EngineError, `threw ${err}`);
      assert(err.payload.kind === 'IllegalModel', `kind ${err.payload.kind}`);
      assert(err.payload.code === 'property-process-relationshipnotype', `code ${err.payload.code}`);
      assert(err.message === 'Relationship dept must have a type', `message ${err.message}`);
      assert(err.payload.modelFile === modelFile, 'the view\'s model file is attached');
    }
    const typed = engine.propertyProcess({
      ast: { $class: `${MM}.RelationshipProperty`, name: 'dept', type: { $class: `${MM}.TypeIdentifier`, name: 'Dept' } },
      getModelFile: () => ({}),
    });
    assert(typed.type === 'Dept', `type ${typed.type}`);
  });

  check('classDeclarationGetProperties resolves any non-null super type, as TS does', () => {
    const own = [{ name: 'own' }];
    const modelFile = { isImportedType: () => false, getType: () => null };
    const view = (superType) => ({
      superType, ast: {}, modelFile, getModelFile: () => modelFile, getOwnProperties: () => own,
    });
    const none = [...engine.classDeclarationGetProperties(view(null))];
    assert(none.length === 1 && none[0] === own[0], `null super type gave ${JSON.stringify(none)}`);
    for (const superType of ['', undefined]) {
      const err = thrown(() => engine.classDeclarationGetProperties(view(superType)));
      assert(err instanceof EngineError && err.payload.kind === 'IllegalModel', `${JSON.stringify(superType)} threw ${err}`);
      assert(err.message === `Could not find super type ${superType}`, `message ${err.message}`);
    }
  });

  // P5-01b (accordproject/concerto-rust#233): `throw()`'s `needsModelFile`
  // flag is the only way a binding with no JS `ModelFile` to attach either
  // way (`modelFileValidateDetached`) can tell its caller apart these two
  // cases, which otherwise look identical (no `modelFile` on the payload at
  // all): an `IllegalModel` error TS itself never attaches a file to (the
  // duplicate-class-name scan, `check_unique_declaration_names`) versus the
  // general case, where `attach_model_file`'s backstop has named this file
  // (even though, at this boundary, there is still no JS object to attach).
  check('modelFileValidateDetached tags needsModelFile (P5-01b)', () => {
    const dup = thrown(() => mm.modelFileValidateDetached(JSON.stringify({
      ...MODEL,
      namespace: 'org.dup@1.0.0',
      declarations: [MODEL.declarations[0], MODEL.declarations[0]],
    }), undefined, 'dup.cto'));
    assert(dup instanceof EngineError, `duplicate class name threw ${dup}`);
    assert(dup.payload.kind === 'IllegalModel', `kind ${dup.payload.kind}`);
    assert(dup.message === 'Duplicate class name org.dup@1.0.0.Person', `message ${dup.message}`);
    assert(dup.payload.needsModelFile === false, `duplicate class name needsModelFile ${dup.payload.needsModelFile}`);
    assert(dup.payload.modelFile === undefined, 'never a JS ModelFile to attach at this boundary either way');

    const broken = thrown(() => mm.modelFileValidateDetached(JSON.stringify(BROKEN), undefined, 'broken.cto'));
    assert(broken instanceof EngineError, `broken super type threw ${broken}`);
    assert(broken.payload.kind === 'IllegalModel', `kind ${broken.payload.kind}`);
    assert(broken.payload.needsModelFile === true, `broken super type needsModelFile ${broken.payload.needsModelFile}`);
    assert(broken.payload.modelFile === undefined, 'still no JS ModelFile to attach at this boundary');
  });

  check('decoratorProcess rejects a null or undefined node (DV-018)', () => {
    const modelFile = { getName: () => 'deco.cto' };
    const view = { getParent: () => ({ getModelFile: () => modelFile }) };
    for (const ast of [null, undefined]) {
      const err = thrown(() => engine.decoratorProcess(ast, view));
      assert(err instanceof EngineError, `${ast} threw ${err}`);
      assert(err.payload.kind === 'IllegalModel', `kind ${err.payload.kind}`);
      assert(err.payload.code === 'decorator-process-notobject', `code ${err.payload.code}`);
      assert(err.message === `Invalid decorator. Expected object. Found ${ast}`, `message ${err.message}`);
      assert(err.payload.modelFile === modelFile, 'the parent\'s model file is attached');
    }
    // An older caller that passes the AST alone still gets the exception.
    const bare = thrown(() => engine.decoratorProcess(null));
    assert(bare instanceof EngineError && bare.payload.code === 'decorator-process-notobject', `bare threw ${bare}`);
    assert(bare.payload.modelFile === undefined, 'no model file without a view');
    // Other non-object nodes do not crash TS: a nameless decorator, as before.
    for (const ast of [5, 'x', {}]) {
      const out = engine.decoratorProcess(ast, view);
      assert(out.name === '' && out.arguments.length === 0, `${JSON.stringify(ast)} gave ${JSON.stringify(out)}`);
    }
    const named = engine.decoratorProcess({ $class: `${MM}.Decorator`, name: 'Hide' }, view);
    assert(named.name === 'Hide', `name ${named.name}`);
  });

  mm.free();
  return rows;
}
