// Model ASTs for the smoke tests and the benchmark.
//
// `syntheticModels` builds a set of namespaces that exercise what
// validateModels walks: imports across namespaces, super types, object and
// relationship properties. Every model it builds validates cleanly.

const MM = 'concerto.metamodel@1.0.0';
const PRIMS = ['String', 'Integer', 'Long', 'Double', 'Boolean', 'DateTime'];

function prop(name, i, localTypes, importedType) {
  const common = { name, isArray: i % 5 === 0, isOptional: i % 3 === 0 };
  if (i % 7 === 3 && importedType) {
    return { $class: `${MM}.RelationshipProperty`, ...common, type: { $class: `${MM}.TypeIdentifier`, name: importedType } };
  }
  if (i % 4 === 1 && localTypes.length) {
    const t = localTypes[i % localTypes.length];
    return { $class: `${MM}.ObjectProperty`, ...common, type: { $class: `${MM}.TypeIdentifier`, name: t } };
  }
  return { $class: `${MM}.${PRIMS[i % PRIMS.length]}Property`, ...common };
}

/** `namespaces` models, each with `decls` declarations of `props` properties. */
export function syntheticModels({ namespaces = 10, decls = 50, props = 10 } = {}) {
  const models = [];
  for (let n = 0; n < namespaces; n++) {
    const ns = `org.spike.n${n}@1.0.0`;
    // Namespace n declares asset Thing<n> and imports Thing<n-1>.
    const imported = n > 0 ? `Thing${n - 1}` : null;
    const declarations = [];
    // Thing<n> is an identified asset, so relationships to it are legal.
    declarations.push({
      $class: `${MM}.AssetDeclaration`, name: `Thing${n}`, isAbstract: false,
      identified: { $class: `${MM}.IdentifiedBy`, name: 'id' },
      properties: [{ $class: `${MM}.StringProperty`, name: 'id', isArray: false, isOptional: false }],
    });
    for (let d = 1; d < decls; d++) {
      const earlier = declarations.filter((x) => x.$class.endsWith('ConceptDeclaration')).map((x) => x.name);
      const decl = {
        $class: `${MM}.ConceptDeclaration`, name: `Type${d}`, isAbstract: d % 10 === 1,
        properties: [],
      };
      if (d % 4 === 2 && earlier.length) {
        decl.superType = { $class: `${MM}.TypeIdentifier`, name: earlier[earlier.length - 1] };
      }
      for (let p = 0; p < props; p++) decl.properties.push(prop(`d${d}p${p}`, p, earlier, imported));
      declarations.push(decl);
    }
    const model = { $class: `${MM}.Model`, namespace: ns, imports: [], declarations };
    if (imported) {
      model.imports.push({ $class: `${MM}.ImportTypes`, namespace: `org.spike.n${n - 1}@1.0.0`, types: [imported] });
    }
    models.push(model);
  }
  return models;
}

/** A model whose super type does not resolve: validateModels must reject it. */
export function invalidModel() {
  return {
    $class: `${MM}.Model`, namespace: 'org.spike.bad@1.0.0', imports: [],
    declarations: [{
      $class: `${MM}.ConceptDeclaration`, name: 'Orphan', isAbstract: false,
      superType: { $class: `${MM}.TypeIdentifier`, name: 'Missing' }, properties: [],
    }],
  };
}
