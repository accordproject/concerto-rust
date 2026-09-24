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
    const ids = [...mm.modelFileIds()];
    assert(ids.length === 1 && ids[0] === 0, `modelFileIds ${ids}`);
    assert(mm.modelFileId('concerto@1.0.0') === 0, 'concerto@1.0.0 is file 0');
    assert(typeof mm.declarationId('concerto@1.0.0.Concept') === 'number', 'Concept has a handle');
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
    const color = mm.declarationId('org.example@1.0.0.Color');
    assert(mm.propertyIds(color).length === 0, 'enum values have no PropId yet (P2-04)');
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
    const dup = thrown(() => mm.addModel(JSON.stringify(MODEL)));
    assert(dup instanceof EngineError, `duplicate namespace threw ${dup}`);
    assert(dup.payload.kind === 'IllegalModel', `kind ${dup.payload.kind}`);
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

  check('init() resolves on a loaded module', () => {
    assert(typeof engine.init === 'function', 'init is exported');
  });

  mm.free();
  return rows;
}
