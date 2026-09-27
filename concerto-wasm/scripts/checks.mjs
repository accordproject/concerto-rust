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

  // accordproject/concerto-rust#217/#219 (superseding an earlier, less
  // accurate version of this check): TS assigns `this.superType =
  // this.ast.superType.name` verbatim (a plain assignment, never a
  // `.toString()` or template-literal coercion), so `this.superType` itself
  // ends up whatever raw JS value that read produces — `undefined` for a
  // `superType` node with no `name` key, `null` for an explicit `name:
  // null`, the number/boolean/array itself for anything else — and
  // `classDeclarationProcess` must reproduce that raw value on its own
  // `superType` field unstringified, not a `.toString()`'d approximation of
  // it. (Whether TS goes on to *resolve* that value into a real declaration
  // — throwing `Could not find super type undefined`/`TypeError:
  // type.startsWith is not a function` along the way for a value that can
  // never resolve — is a separate, later step this function does not take;
  // `classDeclarationGetProperties`/`classDeclarationGetProperty`'s own
  // checks above, and `classDeclarationProcess keeps this.ast.superType.name
  // raw (#219)` below, cover that.) An `identified.name` that is `null`
  // stays `null` for the same reason, read downstream with a plain
  // truthiness check.
  check('classDeclarationProcess tells a missing superType.name from an explicit null apart', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Foo',
      fqn: 'org.example@1.0.0.Foo',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const noName = engine.classDeclarationProcess(mockDeclaration({ superType: {}, properties: [] }));
    assert(
      noName.superType === undefined,
      `superType:{} -> superType ${JSON.stringify(noName.superType)}`,
    );

    const nullName = engine.classDeclarationProcess(
      mockDeclaration({ superType: { name: null }, properties: [] }),
    );
    assert(nullName.superType === null, `superType.name:null -> superType ${JSON.stringify(nullName.superType)}`);

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

  // accordproject/concerto-rust#217/#219: a falsy but non-nullish
  // `identified.name` (`0`, `false`, `""`) is a plain assignment in TS too,
  // read downstream with a truthiness check (`if (this.idField)`), so it
  // must come back as that exact raw falsy value, not a stringified
  // truthy-looking substitute (`"0"`, `"false"`) a property lookup would
  // then wrongly go on to look for.
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
        Object.is(result.idField, name),
        `identified.name:${JSON.stringify(name)} -> idField ${JSON.stringify(result.idField)}`,
      );
      results[JSON.stringify(name)] = result;
    }
    return results;
  });

  // accordproject/concerto-rust#217/#219: same theme, on `superType.name` —
  // a falsy but non-nullish name (`0`, `false`) is a *different* shape from
  // an explicit `null` (the check above): TS's plain assignment leaves
  // `this.superType` as that exact raw falsy value, not `null` and not a
  // stringified `"0"`/`"false"`. Folding it into `null` would make Rust
  // treat "has an unresolvable super type" as "has none at all" — the
  // reverse of this issue's own ts=ok/rust=error shape; stringifying it
  // would let a property lookup resolve a name TS's own resolution can
  // never find.
  check('classDeclarationProcess treats a falsy, non-nullish superType.name as an unresolvable super type', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Employee',
      fqn: 'org.example@1.0.0.Employee',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const results = {};
    for (const name of [0, false]) {
      const result = engine.classDeclarationProcess(
        mockDeclaration({
          superType: { $class: `${MM}.TypeIdentifier`, name },
          properties: [],
        }),
      );
      assert(
        Object.is(result.superType, name),
        `superType.name:${JSON.stringify(name)} -> superType ${JSON.stringify(result.superType)}`,
      );
      results[JSON.stringify(name)] = result;
    }
    return results;
  });

  // accordproject/concerto-rust#217/#219: a *truthy* non-string
  // `superType.name` (an array, a non-zero number, `true`, ...) is the other
  // shape TS never stringifies — `this.superType` stays that raw value.
  // `classDeclarationProcess` itself never resolves or reads it as a string
  // (no `.toString()`/`.startsWith()` call here), so it must not throw for
  // this shape either — only a later resolution step
  // (`ModelFile.getLocalType`'s `type.startsWith(...)`, outside this
  // function) can ever raise `TypeError: type.startsWith is not a function`
  // for it. An earlier version of this check wrongly expected
  // `classDeclarationProcess` itself to raise that `TypeError`.
  check('classDeclarationProcess keeps a truthy non-string superType.name raw, not stringified or resolved', () => {
    const mockDeclaration = (ast) => ({
      ast,
      name: 'Car',
      fqn: 'org.example@1.0.0.Car',
      getModelFile: () => ({ isSystemModelFile: () => false }),
    });

    const result = engine.classDeclarationProcess(
      mockDeclaration({ superType: { name: ['Vehicle'] }, properties: [] }),
    );
    assert(
      Array.isArray(result.superType) && result.superType.length === 1 && result.superType[0] === 'Vehicle',
      `superType.name:["Vehicle"] -> superType ${JSON.stringify(result.superType)}`,
    );

    return { superType: result.superType };
  });

  // accordproject/concerto-rust#219 (P5-05 stage-2 T2c): TS's own check in
  // `MapDeclaration.process` is `if (!this.ast.key || !this.ast.value)` —
  // plain JS truthiness of the whole node — so a fuzz-mutated `key`/`value`
  // of `false`, `0`, `null` or `""` must raise the same "must contain Key &
  // Value properties" message a missing one does, not fall through to
  // `isValidMapKey`/`isValidMapValue`'s own, differently-worded rejection
  // (or, for `null`, crash reading a property of it) the way a check that
  // only excluded JS `undefined` did.
  check('mapDeclarationProcess treats a falsy key or value the same as a missing one', () => {
    const modelFile = { getName: () => 'maps.cto' };
    const mockView = (key, value) => ({
      ast: { name: 'M', key, value },
      modelFile,
      getModelFile: () => modelFile,
    });

    for (const key of [false, 0, '', null]) {
      const err = thrown(() => engine.mapDeclarationProcess(
        mockView(key, { $class: `${MM}.StringMapValueType` }),
      ));
      assert(err instanceof EngineError, `key:${JSON.stringify(key)} threw ${err}`);
      assert(err.payload.code === 'mapdeclaration-process-missingkeyvalue', `key:${JSON.stringify(key)} code ${err.payload.code}`);
      assert(err.message === 'MapDeclaration must contain Key & Value properties M', `key:${JSON.stringify(key)} message ${err.message}`);
    }
    for (const value of [false, 0, '', null]) {
      const err = thrown(() => engine.mapDeclarationProcess(
        mockView({ $class: `${MM}.StringMapKeyType` }, value),
      ));
      assert(err instanceof EngineError, `value:${JSON.stringify(value)} threw ${err}`);
      assert(err.payload.code === 'mapdeclaration-process-missingkeyvalue', `value:${JSON.stringify(value)} code ${err.payload.code}`);
    }
    // Sanity: a genuinely valid key/value still passes.
    engine.mapDeclarationProcess(mockView(
      { $class: `${MM}.StringMapKeyType` },
      { $class: `${MM}.StringMapValueType` },
    ));
  });

  // accordproject/concerto-rust#219 (P5-05 stage-2 T2c): TS's
  // `MapValueType.processType` (mapvaluetype.ts) is `if (!('type' in ast))`
  // then `if (!('$class' in ast.type) || !('name' in ast.type))`. `'type' in
  // ast` is true whenever the key is merely present, even set to `null`, so
  // that must NOT raise the "must contain property 'type'"
  // `IllegalModelException` — it must fall through to the second check,
  // where `ast.type` being anything other than a JS object (`null`, a
  // boolean, a number, a string) makes the `in` operator itself throw a
  // `TypeError`, not an `IllegalModelException`. Verified live against the
  // TS reference (ts-node vs. the built `concerto-wasm` package,
  // `CONCERTO_ENGINE=rust`) for every shape below.
  check("mapValueTypeProcess raises the 'in' operator TypeError for a non-object type, not the missing-property IllegalModelException", () => {
    const parent = { name: 'M' };
    const mockView = (type) => ({
      ast: { $class: `${MM}.ObjectMapValueType`, type },
      parent,
    });

    for (const [type, rendered] of [
      [null, 'null'],
      [true, 'true'],
      [false, 'false'],
      [0, '0'],
      [1e21, '1e+21'],
      ['__proto__', '__proto__'],
      ['', ''],
    ]) {
      const err = thrown(() => engine.mapValueTypeProcess(mockView(type)));
      assert(err instanceof EngineError, `type:${JSON.stringify(type)} threw ${err}`);
      assert(err.payload.kind === 'JsTypeError', `type:${JSON.stringify(type)} kind ${err.payload.kind}`);
      assert(
        err.message === `Cannot use 'in' operator to search for '$class' in ${rendered}`,
        `type:${JSON.stringify(type)} message ${err.message}`,
      );
    }

    // A missing `type` key (not present at all) still raises the ordinary
    // "must contain property 'type'" IllegalModelException.
    const missing = thrown(() => engine.mapValueTypeProcess({ ast: { $class: `${MM}.ObjectMapValueType` }, parent }));
    assert(missing instanceof EngineError, `missing type threw ${missing}`);
    assert(missing.payload.kind === 'IllegalModel', `missing type kind ${missing.payload.kind}`);
    assert(
      missing.message === "ObjectMapValueType must contain property 'type', for MapDeclaration named M",
      `missing type message ${missing.message}`,
    );

    // An array or a plain object (both real JS objects) does not throw a
    // TypeError; it falls through to the "malformed type" rejection.
    for (const type of [[], {}]) {
      const err = thrown(() => engine.mapValueTypeProcess(mockView(type)));
      assert(err instanceof EngineError, `type:${JSON.stringify(type)} threw ${err}`);
      assert(err.payload.kind === 'IllegalModel', `type:${JSON.stringify(type)} kind ${err.payload.kind}`);
      assert(
        err.message === "ObjectMapValueType type must contain property '$class' and property 'name', for MapDeclaration named M",
        `type:${JSON.stringify(type)} message ${err.message}`,
      );
    }

    // Sanity: a genuinely well-formed type still processes.
    const ok = engine.mapValueTypeProcess(mockView({ $class: `${MM}.TypeIdentifier`, name: 'Foo' }));
    assert(ok === 'Foo', `well-formed type -> ${JSON.stringify(ok)}`);
  });

  // accordproject/concerto-rust#219 (P5-05 stage-2 T2c, 103-case residual
  // gap): the same "present, even if null" rule as the outer `type` key
  // above (`!('type' in ast)`) applies to `!('$class' in ast.type) ||
  // !('name' in ast.type)` too — a *present* `ast.type.$class: null` must
  // NOT raise the "must contain property" `IllegalModelException`; it must
  // fall through to the `$class !== 'TypeIdentifier'` check and raise
  // "type $class must be of TypeIdentifier" instead. The fuzz-triage
  // minimised repro `value.type.$class = null` on
  // `gaps/ModelManager.fromAst/15c7357c92f6ae8ae942ab59.json`. Verified
  // live against the TS reference (ts-node vs. the built `concerto-wasm`
  // package, `CONCERTO_ENGINE=rust`).
  check("mapValueTypeProcess treats a present, null type.$class/type.name as present, not missing", () => {
    const parent = { name: 'M' };
    const mockView = (type) => ({
      ast: { $class: `${MM}.ObjectMapValueType`, type },
      parent,
    });

    const nullClass = thrown(() => engine.mapValueTypeProcess(mockView({ $class: null, name: 'Foo' })));
    assert(nullClass instanceof EngineError, `type.$class:null threw ${nullClass}`);
    assert(nullClass.payload.kind === 'IllegalModel', `type.$class:null kind ${nullClass.payload.kind}`);
    assert(
      nullClass.message === "ObjectMapValueType type $class must be of TypeIdentifier for MapDeclaration named M",
      `type.$class:null message ${nullClass.message}`,
    );

    // A present, null `name` passes both presence checks (the `$class` here
    // IS `TypeIdentifier`) and simply stringifies, like TS's own
    // `String(this.ast.type.name)` does for `null` -> `"null"` — it must
    // not throw at all.
    const nullName = engine.mapValueTypeProcess(mockView({ $class: `${MM}.TypeIdentifier`, name: null }));
    assert(nullName === 'null', `type.name:null -> ${JSON.stringify(nullName)}`);
  });

  // accordproject/concerto-rust#219 (P5-05 stage-2 T2c): TS interpolates the
  // raw `this.ast.name` into a template literal in every one of
  // `MapDeclaration.process`'s own messages, which applies JS `ToString` —
  // a missing `name` key stringifies to the literal text `"undefined"`, an
  // explicit `null` to `"null"`, neither to an empty string.
  check('mapDeclarationProcess stringifies a missing or null name like TS, not as empty', () => {
    const modelFile = { getName: () => 'maps.cto' };
    for (const [name, expectedSuffix] of [[undefined, 'undefined'], [null, 'null']]) {
      const ast = { key: false, value: { $class: `${MM}.StringMapValueType` } };
      if (name !== undefined) { ast.name = name; }
      const err = thrown(() => engine.mapDeclarationProcess({ ast, modelFile, getModelFile: () => modelFile }));
      assert(err instanceof EngineError, `name:${JSON.stringify(name)} threw ${err}`);
      assert(
        err.message === `MapDeclaration must contain Key & Value properties ${expectedSuffix}`,
        `name:${JSON.stringify(name)} message ${err.message}`,
      );
    }
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

  // accordproject/concerto-rust#219 (P5-05 stage-2 T2c): TS's own
  // `this.minSize > this.maxSize` (`this.minLength > this.maxLength`)
  // compares the two bounds completely untouched — a fuzz-mutated bound
  // that is itself a non-numeric string can turn this into a *string*
  // comparison (both operands' `ToPrimitive` staying/becoming strings,
  // `[1]` -> `"1"`), not the always-`NaN`, always-`false` comparison
  // converting each side to a number first would give. These are the
  // fuzz-triage minimised repros (`stage2/triage-clusters.json`)
  // `7b9fc1eca8208827732709eb` and `15270a3d46ae76b3adf549eb`.
  check('collectionSizeValidatorNew/stringValidatorNew compare a non-numeric bound with JS\'s untyped >, not a value already coerced to a number', () => {
    const field = { getName: () => 'tags' };
    const decl = { getFullyQualifiedName: () => 'ns.Field.tags' };
    const view = { field, getFieldOrScalarDeclaration: () => decl };

    // `minSize: "aaaa…"`, `maxSize: [1]`: `[1]`'s `ToPrimitive` is `"1"`, so
    // both sides are strings and JS compares them lexicographically
    // (`'a'` > `'1'`), rejecting the model — a plain numeric comparison
    // (`ToNumber("aaaa…")` is `NaN`) would wrongly accept it instead.
    const size = thrown(() => engine.collectionSizeValidatorNew(
      view, { minSize: 'aaaaaaaaaaaaaaaaaaaaa', maxSize: [1] },
    ));
    assert(size instanceof EngineError, `size threw ${size}`);
    assert(
      size.message === 'Validator error for field `tags`. ns.Field.tags: minSize must be less than or equal to maxSize.',
      `size message ${size.message}`,
    );

    // `minLength: "__proto__"`, `maxLength: [10]`: same shape, for
    // `StringValidator`'s length bounds.
    const length = thrown(() => engine.stringValidatorNew(
      view, null, { minLength: '__proto__', maxLength: [10] },
    ));
    assert(length instanceof EngineError, `length threw ${length}`);
    assert(
      length.message === 'Validator error for field `tags`. ns.Field.tags: minLength must be less than or equal to maxLength.',
      `length message ${length.message}`,
    );

    // Sanity: two real, in-order numeric bounds still construct fine.
    const ok = engine.collectionSizeValidatorNew(view, { minSize: 1, maxSize: 10 });
    assert(ok.minSize === 1 && ok.maxSize === 10, `well-formed bounds -> ${JSON.stringify(ok)}`);
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

  // #219 (P5-05 stage-2 T2c review fix, "the location-suffix cluster"):
  // ModelManager.addModelFile's `File '…': line <n> column <n>, to line
  // <n> column <n>.` suffix comes from the `location`/`modelFile` TS's own
  // `IllegalModelException` constructor is given, not from any text Rust
  // appends to the message — so the WASM boundary must actually attach both
  // onto the thrown error's payload for an invalid property name, not just
  // the Rust-only `ContractError` fields property.rs's own unit test checks.
  check('propertyProcess attaches the AST location and model file to an invalid-name error (#219)', () => {
    const modelFile = { getName: () => 'invalidname.cto' };
    const location = {
      $class: `${MM}.Range`,
      start: { $class: `${MM}.Position`, line: 3, column: 5, offset: 20 },
      end: { $class: `${MM}.Position`, line: 3, column: 30, offset: 45 },
    };
    const ast = {
      $class: `${MM}.StringProperty`, name: 1e308, isArray: false, isOptional: false, location,
    };
    const err = thrown(() => engine.propertyProcess({ ast, getModelFile: () => modelFile }));
    assert(err instanceof EngineError, `threw ${err}`);
    assert(err.payload.kind === 'IllegalModel', `kind ${err.payload.kind}`);
    assert(err.message.includes("Invalid property name '1e+308'"), `message ${err.message}`);
    assert(err.payload.modelFile === modelFile, 'the view\'s model file is attached');
    assert(
      JSON.stringify(err.payload.location) === JSON.stringify(location),
      `location ${JSON.stringify(err.payload.location)}`,
    );
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

  // #219 (P5-05 stage-2 T2c): classDeclarationGetProperty (singular) has the
  // same `this.superType !== null` guard as GetProperties above, not
  // truthiness — an empty-string or `undefined` super type still goes on to
  // be resolved, not treated as "no super type".
  check('classDeclarationGetProperty resolves any non-null super type, as TS does', () => {
    const superProp = { name: 'inherited' };
    const superClass = { getProperty: (n) => (n === 'x' ? superProp : null) };
    const modelFile = { isImportedType: () => false, getType: () => superClass };
    const view = (superType) => ({
      superType, ast: {}, modelFile, getModelFile: () => modelFile,
      getOwnProperty: () => null,
    });
    const none = engine.classDeclarationGetProperty(view(null), 'x');
    assert(none === null, `null super type gave ${JSON.stringify(none)}`);
    for (const superType of ['', undefined]) {
      const found = engine.classDeclarationGetProperty(view(superType), 'x');
      assert(found === superProp, `${JSON.stringify(superType)} super type did not resolve: ${JSON.stringify(found)}`);
    }
  });

  // #219 (P5-05 stage-2 T2c): classDeclarationProcess never coerces
  // `this.ast.superType.name` — the raw AST value (including `undefined`
  // when the AST names a super type but the AST's own `.name` is absent)
  // survives unstringified into the `superType` decision, and only a
  // genuinely falsy *superType node itself* (no `superType` at all, or one
  // that is itself falsy) falls back to the implicit `'Concept'`.
  check('classDeclarationProcess keeps this.ast.superType.name raw (#219)', () => {
    const modelFile = { isSystemModelFile: () => false };
    const declaration = (superType) => ({
      ast: superType === undefined ? {} : { superType },
      name: 'Foo',
      fqn: 'test@1.0.0.Foo',
      getModelFile: () => modelFile,
    });
    const cases = [
      // superType node present, but its own `.name` is absent/null/falsy:
      // kept raw, never coerced or defaulted to 'Concept'.
      [{ $class: `${MM}.TypeIdentifier` }, undefined],
      [{ $class: `${MM}.TypeIdentifier`, name: null }, null],
      [{ $class: `${MM}.TypeIdentifier`, name: false }, false],
      [{ $class: `${MM}.TypeIdentifier`, name: 'Bar' }, 'Bar'],
      // superType node itself falsy (or absent): TS's outer truthiness
      // test fails, so the implicit 'Concept' applies.
      [false, 'Concept'],
      [undefined, 'Concept'],
    ];
    for (const [superType, expected] of cases) {
      const decision = engine.classDeclarationProcess(declaration(superType));
      assert(
        Object.is(decision.superType, expected),
        `superType ${JSON.stringify(superType)} gave ${JSON.stringify(decision.superType)}, want ${JSON.stringify(expected)}`,
      );
    }
  });

  // #219 (P5-05 stage-2 T2c review fix): classDeclarationProcess never
  // coerces `this.ast.identified.name` either — TS's `this.idField =
  // this.ast.identified.name` is a plain assignment, so a falsy raw value
  // (absent, `null`, `false`) must stay that raw value, not the *string*
  // "undefined"/"null"/"false" (which would make `ClassDeclaration.validate`'s
  // `if (this.idField)` guard wrongly true and reject a model TS accepts).
  check('classDeclarationProcess keeps a falsy this.ast.identified.name raw, not stringified (#219)', () => {
    const modelFile = { isSystemModelFile: () => false };
    const declaration = (identified) => ({
      ast: { identified },
      name: 'Foo',
      fqn: 'test@1.0.0.Foo',
      getModelFile: () => modelFile,
    });
    const cases = [
      [{ $class: `${MM}.IdentifiedBy` }, undefined],
      [{ $class: `${MM}.IdentifiedBy`, name: null }, null],
      [{ $class: `${MM}.IdentifiedBy`, name: false }, false],
      [{ $class: `${MM}.IdentifiedBy`, name: 'id' }, 'id'],
    ];
    for (const [identified, expected] of cases) {
      const decision = engine.classDeclarationProcess(declaration(identified));
      assert(
        Object.is(decision.idField, expected),
        `identified ${JSON.stringify(identified)} gave idField ${JSON.stringify(decision.idField)}, want ${JSON.stringify(expected)}`,
      );
      assert(decision.addIdentifierField === false, 'addIdentifierField is false for an explicit IdentifiedBy');
    }
    // The system-identified branch (`identified.$class` is not IdentifiedBy)
    // is unaffected: idField is always the literal '$identifier'.
    const systemIdentified = engine.classDeclarationProcess(
      declaration({ $class: `${MM}.Identified` }),
    );
    assert(systemIdentified.idField === '$identifier', `system-identified idField ${systemIdentified.idField}`);
    assert(systemIdentified.addIdentifierField === true, 'addIdentifierField is true for system identification');
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
    // Other non-object nodes do not crash TS: a nameless decorator, as
    // before. TS's own `this.name = ast.name` is a plain assignment, so a
    // node with no `name` key leaves it genuinely `undefined` — not the
    // empty string an earlier version of this check wrongly expected
    // (accordproject/concerto-rust#219 review: fixing the "Duplicate
    // decorator" cluster's `decoratorProcess` also fixed the value this
    // returns for every nameless decorator, not only the ones that collide).
    for (const ast of [5, 'x', {}]) {
      const out = engine.decoratorProcess(ast, view);
      assert(out.name === undefined && out.arguments.length === 0, `${JSON.stringify(ast)} gave ${JSON.stringify(out)}`);
    }
    const named = engine.decoratorProcess({ $class: `${MM}.Decorator`, name: 'Hide' }, view);
    assert(named.name === 'Hide', `name ${named.name}`);
  });

  // #219 T2c continuation brief (2026-09-26 23:20): the fuzz-triage 5-case
  // regression on seed conformance/ModelManager.addModelFile/
  // 0c7acbac5ee755a471e0801f.json, minimised (stage2/triage-clusters.json)
  // to two edits on that seed's AST — a `null` second entry in
  // `declarations[0].properties[0].decorators` (alongside one well-formed
  // `custom` decorator) and `declarations[0].identified.$class` set to
  // `true` — reproduced here through the two bindings TS's own
  // `ClassDeclaration.process`/`Decorated.process` actually call, in TS's
  // own order: the class's own superType/identified decision
  // (`classDeclarationProcess`) runs first — `this.ast.identified.$class
  // === '...IdentifiedBy'` is a strict-equality check, never a `.toString()`
  // call, so a boolean `$class` simply fails to match, same as any other
  // non-matching value, and must not crash the way the pre-#217/#218
  // rust-mode bug did (`this.ast.identified.$class.toString is not a
  // function`) — then TS iterates `ast.properties`, building each
  // property's own view, whose `Decorated.process` calls `decoratorProcess`
  // once per decorator: the first (well-formed) decorator processes
  // normally, and the second, `null`, throws DV-018's
  // `IllegalModelException` (`decorator-process-notobject`) — never TS's
  // own crash, which is the divergence DV-018 accepts. Before the #217/#218
  // merge (4d7be66) rust mode instead threw its own bogus
  // `this.ast.identified.$class.toString is not a function` TypeError for
  // this exact combination; this pins that the regression is resolved, not
  // "unaddressed" (a prior turn's handoff comment wrongly reported no repro
  // or fix for this case).
  check('identified.$class=true beside a null decorator resolves via DV-018, not a toString crash (#219 5-case regression)', () => {
    const modelFile = { isSystemModelFile: () => false, getName: () => 'decorated_001_duplicate_decorator.json' };
    const declaration = {
      ast: { identified: { $class: true, name: 'productId' } },
      name: 'Product',
      fqn: 'org.example.decorated001.invalid@1.0.0.Product',
      getModelFile: () => modelFile,
    };
    // classDeclarationProcess must not crash reading a non-string
    // `identified.$class` — the old bug's own site.
    const decision = engine.classDeclarationProcess(declaration);
    assert(typeof decision === 'object' && decision !== null, `decision ${JSON.stringify(decision)}`);

    // The property's own decorators, processed in TS's order: the first is
    // well-formed, the second (mirroring the fixture) is `null`.
    const view = { getParent: () => ({ getModelFile: () => modelFile }) };
    const first = engine.decoratorProcess({ $class: `${MM}.Decorator`, name: 'custom' }, view);
    assert(first.name === 'custom', `first decorator name ${first.name}`);
    const err = thrown(() => engine.decoratorProcess(null, view));
    assert(err instanceof EngineError, `null decorator threw ${err}`);
    assert(err.payload.kind === 'IllegalModel', `kind ${err.payload.kind}`);
    assert(err.payload.code === 'decorator-process-notobject', `code ${err.payload.code}`);
    assert(err.message === 'Invalid decorator. Expected object. Found null', `message ${err.message}`);
  });

  mm.free();
  return rows;
}
