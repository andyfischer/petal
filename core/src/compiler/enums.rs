//! Enums: declaration checks, the enum's name as a binding, and the static
//! resolution of `Enum.variant` in expressions and patterns.
//!
//! An `enum` is a top-level, hoisted declaration, like a `class`. Its name is
//! the only binding it introduces; a variant is a *member* of that name
//! (`Color.red`), never a name of its own.
//!
//! - **The binding.** The enum's name is bound, ahead of the file's first
//!   statement, to a term that stands for the declaration. `pub`, imports and
//!   `mod.Color` carry that term by the ordinary binding rules, and a name
//!   that resolves to it *is* that enum — so the compiler knows the
//!   declaration wherever the name is in scope.
//! - **Members.** `Color.red` compiles straight to the variant,
//!   `Color.custom(1, 2, 3)` to the construction, `Color.all` to the list and
//!   `Color.from(s)` to the lookup; `Color.rde` is a compile error. Nothing
//!   is emitted for the declaration itself, so an enum costs a run nothing
//!   until it is used.
//! - **The name as a value.** Read on its own (`let E = Color`), the name is
//!   a record of its members — each variant, `all` and `from` — built where
//!   it is read. A member of such a value (`E.red`) is an ordinary field read.
//! - **The value.** A variant value carries a tag — `"Color.red"`, enum
//!   names being unique in a compilation — and its payload. Payload field
//!   names live in [`Program::enums`]; constant data (`key: expr`) is
//!   evaluated once per run, after the file's hoisted declarations, and
//!   published with [`ENUM_DATA_BUILTIN`].

use super::*;

/// Member names an enum's own record defines, which a variant may not take.
const RESERVED_VARIANTS: [&str; 2] = ["all", "from"];
/// The field every variant value answers (`k.name`), which a payload field or
/// a data key may not take.
const RESERVED_FIELD: &str = "name";

/// What a data expression cannot use, while an enum's constant data is being
/// compiled (see [`Compiler::emit_enum_data`]).
pub(super) struct EnumDataScope {
    /// Top-level names the file binds only as it runs.
    pub(super) unavailable: HashSet<String>,
    /// The bindings of this file's functions that are not hoisted, and so
    /// are still empty where the data is evaluated.
    pub(super) late_fns: HashSet<TermId>,
}

/// A compile-wide enum declaration: its shape, and the file that declared it.
pub(super) struct EnumInfo {
    pub(super) def: EnumDef,
    origin: Option<String>,
    /// The declaring statement, to tell a redeclaration from the declaration.
    span: SourceSpan,
}

impl Compiler {
    /// Register this module's `enum` declarations and report what is wrong
    /// with them. Runs with the rest of the prescan, so a variant is known
    /// wherever in the file (or an importer) it is named.
    pub(super) fn prescan_enums(&mut self, stmts: &[Stmt], module: Option<&str>) {
        for (span, name) in nested_enum_decls(stmts) {
            self.error_at(
                span,
                format!(
                    "enum `{name}` must be declared at the top level of a file, not inside \
                     a function, loop or block"
                ),
            );
        }
        for stmt in stmts {
            let StmtKind::EnumDecl { name, variants } = &stmt.kind else {
                continue;
            };
            if let Some(message) = self.enum_name_conflict(name, module) {
                self.error_at(stmt.span, message);
                continue;
            }
            for (span, message) in check_enum_variants(name, variants) {
                self.error_at(span, message);
            }
            // A variant named `all` or `from` was just reported; the enum's
            // own member keeps the name, so no later code sees the variant.
            let mut def = EnumDef::declared(variants);
            def.variants
                .retain(|v| !RESERVED_VARIANTS.contains(&v.name.as_str()));
            // The type `name` in an annotation, and what the checker knows a
            // value of it answers.
            self.classes.declare_enum(name, def.clone());
            self.enum_defs.insert(
                name.clone(),
                EnumInfo {
                    def,
                    origin: module.map(str::to_string),
                    span: stmt.span,
                },
            );
        }
    }

    /// Why `name` cannot be declared as an enum here, if it cannot. The rules
    /// are the class rules: one declaration of a name per compilation, and no
    /// built-in type name.
    fn enum_name_conflict(&self, name: &str, module: Option<&str>) -> Option<String> {
        if crate::types::Type::from_name(name).is_some() {
            return Some(format!(
                "enum `{name}` collides with the built-in type name `{name}`: pick another name"
            ));
        }
        if let Some(prev) = self.enum_defs.get(name) {
            return Some(match (&prev.origin, module) {
                (Some(prev), Some(now)) if prev != now => format!(
                    "enum `{name}` is already declared in `{prev}`, so `{now}` may not \
                     declare it too"
                ),
                _ => format!("enum `{name}` is already declared"),
            });
        }
        let class = self.classes.lookup_anywhere(name)?;
        (!self.classes.get(class).builtin).then(|| {
            format!("enum `{name}` has the same name as class `{name}`: pick another name")
        })
    }

    /// Whether `stmt` is the registered declaration of the enum `name` —
    /// false for a redeclaration, which was reported and declares nothing.
    fn is_enum_declaration(&self, name: &str, stmt: &Stmt) -> bool {
        self.enum_defs
            .get(name)
            .is_some_and(|info| info.span == stmt.span)
    }

    /// Bind an enum's name, ahead of the file's first statement, to the term
    /// that stands for its declaration. The term computes nothing: every use
    /// of the name is compiled from the declaration (see the module docs).
    pub(super) fn bind_enum(&mut self, stmt: &Stmt) {
        let StmtKind::EnumDecl { name, .. } = &stmt.kind else {
            return;
        };
        if !self.is_enum_declaration(name, stmt) {
            return;
        }
        let tid = self.emit_phantom_term(name.clone());
        self.source_map.add(tid, stmt.span);
        self.enum_terms.insert(tid, name.clone());
        self.scope_bind(name.clone(), tid);
    }

    /// The enum `name` is bound to, when its binding is an enum's own.
    pub(super) fn enum_bound_to(&self, name: &str) -> Option<String> {
        self.enum_terms.get(&self.scope_lookup(name)?).cloned()
    }

    /// An enum read as a value: the record of its members — each variant
    /// under its own name (the value, or its constructor), then `all` and
    /// `from`.
    pub(super) fn emit_enum_record(&mut self, enum_name: &str) -> TermId {
        let variants: Vec<String> = self
            .enum_defs
            .get(enum_name)
            .map(|info| info.def.variants.iter().map(|v| v.name.clone()).collect())
            .unwrap_or_default();
        let mut keys: Vec<ConstantId> = Vec::new();
        let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
        for member in variants.iter().map(String::as_str).chain(RESERVED_VARIANTS) {
            if let Some(tid) = self.emit_enum_member(enum_name, member) {
                keys.push(self.string_constant(member));
                inputs.push(tid);
            }
        }
        self.emit_term(
            TermOp::AllocMap {
                fields: keys,
                class: None,
            },
            inputs,
            None,
        )
    }

    /// One member of an enum, as a value: a payload-free variant, a payload
    /// variant's constructor, the `all` list or the `from` function. `None`
    /// when the enum has no such member.
    fn emit_enum_member(&mut self, enum_name: &str, member: &str) -> Option<TermId> {
        Some(match self.enum_variant_def(enum_name, member) {
            Some(v) if v.fields.is_empty() => self.emit_enum_variant(enum_name, member, smallvec![]),
            Some(v) => self.compile_enum_constructor(enum_name, &v),
            None if member == "all" => {
                let plain: Vec<String> = self
                    .enum_defs
                    .get(enum_name)
                    .map(|info| {
                        let plain = info.def.variants.iter().filter(|v| v.fields.is_empty());
                        plain.map(|v| v.name.clone()).collect()
                    })
                    .unwrap_or_default();
                let inputs = plain
                    .iter()
                    .map(|v| self.emit_enum_variant(enum_name, v, smallvec![]))
                    .collect();
                self.emit_term(TermOp::AllocList, inputs, None)
            }
            None if member == "from" => self.compile_enum_from(enum_name),
            None => return None,
        })
    }

    /// Emit the constant data of every enum this file declares: one record
    /// per variant that has any, published under the variant's tag.
    ///
    /// Emitted after the file's hoisted constants and functions, so a data
    /// expression may use them — and may name a variant of any enum, since
    /// every enum's name is bound by then. A name the file only computes
    /// later (`let`, `var`, `state`) is not available, and neither is a
    /// function (`late_fns`) that reads one; see `compile_ident`.
    ///
    /// Data that reads another variant's data (`K.a.cost + 1`) is emitted
    /// after it, wherever the two are declared.
    pub(super) fn emit_enum_data(
        &mut self,
        stmts: &[Stmt],
        unavailable: HashSet<String>,
        late_fns: &HashSet<String>,
    ) {
        let mut items: Vec<(&str, &EnumVariant)> = Vec::new();
        for stmt in stmts {
            if let StmtKind::EnumDecl { name, variants } = &stmt.kind
                && self.is_enum_declaration(name, stmt)
            {
                items.extend(variants.iter().filter(|v| !v.data.is_empty()).map(|v| (name.as_str(), v)));
            }
        }
        if items.is_empty() {
            return;
        }
        let tags: Vec<String> = items
            .iter()
            .map(|(name, v)| enum_variant_tag(name, &v.name))
            .collect();
        let reads: Vec<Vec<usize>> = items
            .iter()
            .map(|(_, v)| {
                let read = self.enum_data_reads(v);
                (0..tags.len()).filter(|&i| read.contains(&tags[i])).collect()
            })
            .collect();

        // Depth-first over what each variant's data reads, in source order.
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Open,
            Done,
        }
        let mut marks = vec![Mark::New; items.len()];
        let mut order: Vec<usize> = Vec::with_capacity(items.len());
        let mut cycles: Vec<(usize, usize)> = Vec::new();
        for root in 0..items.len() {
            // (variant, how many of its reads were followed).
            let mut path: Vec<(usize, usize)> = vec![(root, 0)];
            if marks[root] != Mark::New {
                continue;
            }
            marks[root] = Mark::Open;
            while let Some((at, next)) = path.last_mut() {
                let at = *at;
                match reads[at].get(*next).copied() {
                    Some(dep) => {
                        *next += 1;
                        match marks[dep] {
                            Mark::New => {
                                marks[dep] = Mark::Open;
                                path.push((dep, 0));
                            }
                            Mark::Open => cycles.push((dep, at)),
                            Mark::Done => {}
                        }
                    }
                    None => {
                        marks[at] = Mark::Done;
                        order.push(at);
                        path.pop();
                    }
                }
            }
        }
        for (first, second) in cycles {
            let message = if first == second {
                format!("the constant data of `{}` reads itself", tags[first])
            } else {
                format!(
                    "the constant data of `{}` and `{}` read each other, so neither can be                      evaluated first",
                    tags[first], tags[second]
                )
            };
            self.error_at(items[first].1.span, message);
        }

        let late_fns = late_fns.iter().filter_map(|n| self.scope_lookup(n)).collect();
        self.enum_data_scope = Some(EnumDataScope {
            unavailable,
            late_fns,
        });
        for i in order {
            let (name, variant) = items[i];
            let mut keys: Vec<ConstantId> = Vec::new();
            let mut inputs: SmallVec<[TermId; 4]> = SmallVec::new();
            for d in &variant.data {
                keys.push(self.string_constant(&d.key));
                inputs.push(self.compile_expr(&d.value));
            }
            let record = self.emit_term(
                TermOp::AllocMap {
                    fields: keys,
                    class: None,
                },
                inputs,
                None,
            );
            let tag = self.string_constant(&enum_variant_tag(name, &variant.name));
            let tag_tid = self.emit_term(TermOp::Constant(tag), smallvec![], None);
            let builtin = self.string_constant(ENUM_DATA_BUILTIN);
            let call = self.emit_call_term(
                TermOp::BuiltinCall(builtin),
                smallvec![tag_tid, record],
                ENUM_DATA_BUILTIN,
                SmallVec::new(),
            );
            self.source_map.add(call, variant.span);
        }
        self.enum_data_scope = None;
    }

    /// The tags of the variants whose constant data `variant`'s data reads
    /// where the compiler can see it: `Enum.variant.key`, written out.
    fn enum_data_reads(&self, variant: &EnumVariant) -> HashSet<String> {
        struct Reads<'a>(&'a Compiler, HashSet<String>);
        impl crate::ast::ExprVisitor for Reads<'_> {
            fn visit_expr(&mut self, e: &Expr) {
                if let ExprKind::FieldAccess { object, field: key } = &e.kind
                    && let ExprKind::FieldAccess { object: of, field } = &object.kind
                    && let Some(enum_name) = self.0.static_enum(of)
                    && let Some(v) = self.0.enum_variant_def(&enum_name, field)
                    && v.data.contains(key)
                {
                    self.1.insert(enum_variant_tag(&enum_name, field));
                }
                crate::ast::walk_expr(self, e);
            }
        }
        let mut reads = Reads(self, HashSet::new());
        for d in &variant.data {
            crate::ast::ExprVisitor::visit_expr(&mut reads, &d.value);
        }
        reads.1
    }

    pub(super) fn string_constant(&mut self, s: &str) -> ConstantId {
        self.constants.intern(ConstantValue::String(s.to_string()))
    }

    /// Emit the construction of `Enum.variant` from its payload terms.
    fn emit_enum_variant(
        &mut self,
        enum_name: &str,
        variant: &str,
        payload: SmallVec<[TermId; 4]>,
    ) -> TermId {
        let tag = self.string_constant(&enum_variant_tag(enum_name, variant));
        self.emit_term(TermOp::MakeEnumVariant(tag), payload, None)
    }

    /// `Enum.from` as a function value: one parameter, forwarded to the
    /// builtin with the enum's name.
    fn compile_enum_from(&mut self, enum_name: &str) -> TermId {
        let params = ["name".to_string()];
        let (body_block, saved_block) = self.begin_function_scope(&params, 0);
        let arg = self.scope_lookup("name").expect("parameter");
        self.emit_enum_from(enum_name, arg);
        self.end_function_scope(
            Some(format!("{enum_name}.from")),
            &params,
            0,
            body_block,
            saved_block,
            None,
        )
    }

    fn emit_enum_from(&mut self, enum_name: &str, arg: TermId) -> TermId {
        let name_c = self.string_constant(enum_name);
        let name_tid = self.emit_term(TermOp::Constant(name_c), smallvec![], None);
        let builtin = self.string_constant(ENUM_FROM_BUILTIN);
        self.emit_call_term(
            TermOp::BuiltinCall(builtin),
            smallvec![name_tid, arg],
            ENUM_FROM_BUILTIN,
            SmallVec::new(),
        )
    }

    // -----------------------------------------------------------------------
    // Static resolution
    // -----------------------------------------------------------------------

    /// The enum `expr` names, when that is known here: a name (`Color`) or a
    /// module member (`mod.Color`) whose binding is an enum's own. A local
    /// that shadows the name, or a value merely holding the enum's record
    /// (`let E = Color`), is not — a member of one is an ordinary field read.
    pub(super) fn static_enum(&self, expr: &Expr) -> Option<String> {
        let tid = match &expr.kind {
            ExprKind::Ident(name) => self.scope_lookup(name)?,
            ExprKind::FieldAccess { object, field } => self.module_member_term(object, field)?,
            _ => return None,
        };
        self.enum_terms.get(&tid).cloned()
    }

    /// [`Self::static_enum`] for a written path: `["Color"]` or
    /// `["mod", "Color"]`, as a pattern spells it.
    fn static_enum_path(&self, path: &[&str]) -> Option<String> {
        let tid = match path {
            [name] => self.scope_lookup(name)?,
            [alias, name] => {
                let module = self.module_aliases.get(*alias)?;
                let tid = self.scope_lookup(&format!("{module}::{name}"))?;
                if let Some(bound) = self.scope_lookup(alias)
                    && self.alias_floor.get(*alias) != Some(&Some(bound))
                {
                    return None;
                }
                tid
            }
            _ => return None,
        };
        self.enum_terms.get(&tid).cloned()
    }

    /// `Enum.member` in value position: the variant, a payload variant's
    /// constructor, `all` or `from`. A member the enum does not have is a
    /// compile error.
    pub(super) fn compile_enum_member(
        &mut self,
        enum_name: &str,
        member: &str,
        span: SourceSpan,
    ) -> TermId {
        match self.emit_enum_member(enum_name, member) {
            Some(tid) => tid,
            None => self.unknown_variant(enum_name, member, span),
        }
    }

    /// `Enum.member(args)`: a construction, or `Enum.from(name)`. A call with
    /// named arguments (or of anything else the enum has) is an ordinary call
    /// of the member's value.
    pub(super) fn compile_enum_call(
        &mut self,
        enum_name: &str,
        member: &str,
        args: &[Expr],
        names: SmallVec<[Option<ConstantId>; 4]>,
        span: SourceSpan,
    ) -> TermId {
        let positional = names.iter().all(Option::is_none);
        match self.enum_variant_def(enum_name, member) {
            // `Event.none()` is `Event.none`: a payload-free variant may be
            // written with empty parens, as its declaration may.
            Some(v) if positional => {
                if args.len() != v.fields.len() {
                    let takes = if v.fields.is_empty() {
                        "has no payload, so it takes no arguments".to_string()
                    } else {
                        format!(
                            "takes {} ({})",
                            count_noun(v.fields.len(), "argument"),
                            v.fields.join(", ")
                        )
                    };
                    self.error_at(
                        span,
                        format!("{enum_name}.{member} {takes}, got {}", args.len()),
                    );
                }
                let payload = args.iter().map(|a| self.compile_expr(a)).collect();
                self.emit_enum_variant(enum_name, member, payload)
            }
            None if member == "from" && positional && args.len() == 1 => {
                let arg = self.compile_expr(&args[0]);
                self.emit_enum_from(enum_name, arg)
            }
            _ => {
                let callee = self.compile_enum_member(enum_name, member, span);
                let mut inputs: SmallVec<[TermId; 4]> = smallvec![callee];
                for arg in args {
                    inputs.push(self.compile_expr(arg));
                }
                let text = enum_variant_tag(enum_name, member);
                self.emit_call_term(TermOp::Call, inputs, &text, names)
            }
        }
    }

    /// The variant names of the enum `enum_name`, in declaration order.
    pub(super) fn enum_variant_names(&self, enum_name: &str) -> Option<Vec<String>> {
        let info = self.enum_defs.get(enum_name)?;
        Some(info.def.variants.iter().map(|v| v.name.clone()).collect())
    }

    fn enum_variant_def(&self, enum_name: &str, variant: &str) -> Option<EnumVariantDef> {
        self.enum_defs.get(enum_name)?.def.variant(variant).cloned()
    }

    /// Report `Enum.member` naming no variant, and stand an error term in for
    /// its value so compilation carries on.
    fn unknown_variant(&mut self, enum_name: &str, member: &str, span: SourceSpan) -> TermId {
        let message = self.unknown_variant_message(enum_name, member);
        self.error_at(span, message.clone());
        let cid = self.constants.intern(ConstantValue::String(message));
        self.emit_term(TermOp::Error(cid), smallvec![], None)
    }

    fn unknown_variant_message(&self, enum_name: &str, member: &str) -> String {
        let names: Vec<&str> = self
            .enum_defs
            .get(enum_name)
            .map(|info| info.def.variants.iter().map(|v| v.name.as_str()).collect())
            .unwrap_or_default();
        match closest_name(member, &names) {
            Some(close) => format!(
                "enum `{enum_name}` has no variant `{member}` — did you mean {enum_name}.{close}?"
            ),
            None => format!(
                "enum `{enum_name}` has no variant `{member}` (its variants: {})",
                names.join(", ")
            ),
        }
    }

    /// `did you mean Color.red?` for a bare name that some enum in scope
    /// declares as a variant; `None` otherwise. A variant is only ever named
    /// through its enum, so this is the likeliest slip behind an undefined
    /// name. `payload` says, where the use shows it, whether the variant
    /// meant carries one (`road(n)` does): a variant of that shape is
    /// suggested ahead of a same-named one of the other.
    pub(super) fn bare_variant_hint(&self, name: &str, payload: Option<bool>) -> Option<String> {
        // Every spelling in scope that reaches an enum declaring `name`,
        // with whether its shape is the other one.
        let mut spellings: Vec<(bool, String)> = Vec::new();
        let other_shape = |v: EnumVariantDef| payload.is_some_and(|p| p == v.fields.is_empty());
        for scope in &self.scopes {
            for (bound, tid) in scope {
                let Some(enum_name) = self.enum_terms.get(tid) else {
                    continue;
                };
                let Some(v) = self.enum_variant_def(enum_name, name) else {
                    continue;
                };
                // A module member is in the global scope as `module::Enum`;
                // only a name the file can write is worth suggesting.
                if !bound.contains("::") {
                    spellings.push((other_shape(v), format!("{bound}.{name}")));
                }
            }
        }
        for (alias, module) in &self.module_aliases {
            for (enum_name, _) in self.enum_defs.iter() {
                if let Some(v) = self.enum_variant_def(enum_name, name)
                    && self
                        .scope_lookup(&format!("{module}::{enum_name}"))
                        .is_some_and(|tid| self.enum_terms.contains_key(&tid))
                {
                    spellings.push((other_shape(v), format!("{alias}.{enum_name}.{name}")));
                }
            }
        }
        // Of the right shape first, then the shortest: the one the file most
        // plausibly meant.
        spellings.sort_by(|a, b| (a.0, a.1.len(), &a.1).cmp(&(b.0, b.1.len(), &b.1)));
        spellings.dedup();
        let spellings: Vec<String> = spellings.into_iter().map(|(_, s)| s).collect();
        match spellings.as_slice() {
            [] => None,
            [only] => Some(format!("did you mean {only}?")),
            [first, rest @ ..] => Some(format!(
                "did you mean {first}? (also {})",
                rest.join(", ")
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Patterns
    // -----------------------------------------------------------------------

    /// Resolve the variant patterns in `pattern` to the tags their values
    /// carry: `Kind.park` and `mod.Kind.park` both become the variant
    /// pattern `Kind.park`, the enum's declared name. A pattern that names no
    /// variant is a compile error.
    pub(super) fn resolve_pattern(&mut self, pattern: &Pattern, span: SourceSpan) -> Pattern {
        match pattern {
            Pattern::Variant { name, fields } => {
                let fields: Vec<Pattern> = fields
                    .iter()
                    .map(|f| self.resolve_pattern(f, span))
                    .collect();
                let name = self.resolve_variant_pattern(name, fields.len(), span);
                Pattern::Variant { name, fields }
            }
            Pattern::List { elements, rest } => Pattern::List {
                elements: elements
                    .iter()
                    .map(|e| self.resolve_pattern(e, span))
                    .collect(),
                rest: rest.clone(),
            },
            Pattern::Record(fields) => Pattern::Record(
                fields
                    .iter()
                    .map(|(k, p)| (k.clone(), self.resolve_pattern(p, span)))
                    .collect(),
            ),
            Pattern::Or(alternatives) => Pattern::Or(
                alternatives
                    .iter()
                    .map(|a| self.resolve_pattern(a, span))
                    .collect(),
            ),
            Pattern::Wildcard | Pattern::Literal(_) | Pattern::Variable(_) => pattern.clone(),
        }
    }

    fn resolve_variant_pattern(&mut self, written: &str, fields: usize, span: SourceSpan) -> String {
        let Some((path, variant)) = written.rsplit_once('.') else {
            // `Circle(r)`: a call-shaped pattern with no enum in front.
            let hint = match self.bare_variant_hint(written, Some(fields > 0)) {
                Some(hint) => format!(" — {hint}"),
                None => String::new(),
            };
            self.error_at(
                span,
                format!(
                    "`{written}(…)` is not a pattern: a variant is matched through its enum, \
                     as `Enum.{written}(…)`{hint}"
                ),
            );
            return written.to_string();
        };
        let segments: Vec<&str> = path.split('.').collect();
        let Some(enum_name) = self.static_enum_path(&segments) else {
            self.error_at(
                span,
                format!(
                    "`{written}` is not a variant pattern: `{path}` is not an enum in scope \
                     (a variant is written `Enum.variant`)"
                ),
            );
            return written.to_string();
        };
        match self.enum_variant_def(&enum_name, variant) {
            None => {
                let message = self.unknown_variant_message(&enum_name, variant);
                self.error_at(span, message);
            }
            Some(v) if v.fields.len() != fields => {
                let shape = if v.fields.is_empty() {
                    "has no payload".to_string()
                } else {
                    format!(
                        "has {} ({}), so its pattern is `{written}({})`",
                        count_noun(v.fields.len(), "payload field"),
                        v.fields.join(", "),
                        v.fields.iter().map(|_| "_").collect::<Vec<_>>().join(", ")
                    )
                };
                self.error_at(
                    span,
                    format!(
                        "the pattern `{written}{}` has {}, but {enum_name}.{variant} {shape}",
                        if fields == 0 { "" } else { "(…)" },
                        count_noun(fields, "field"),
                    ),
                );
            }
            Some(_) => {}
        }
        enum_variant_tag(&enum_name, variant)
    }

    /// Every enum this compilation declared, as the program records them.
    pub(super) fn take_enum_defs(&mut self) -> std::collections::BTreeMap<String, EnumDef> {
        std::mem::take(&mut self.enum_defs)
            .into_iter()
            .map(|(name, info)| (name, info.def))
            .collect()
    }
}

fn count_noun(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The candidate closest to `word` by edit distance, if one is close enough
/// to be a plausible misspelling of it.
fn closest_name<'a>(word: &str, candidates: &[&'a str]) -> Option<&'a str> {
    // Two edits cover a transposition (`rde` for `red`); a one- or
    // two-letter name is too short for a near miss to mean anything.
    let budget = match word.chars().count() {
        0 | 1 => return None,
        2 => 1,
        n => (n / 3).max(2),
    };
    candidates
        .iter()
        .map(|c| (crate::diagnostic::edit_distance(word, c), *c))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// What is wrong with an enum's variants, each with the span to blame.
fn check_enum_variants(enum_name: &str, variants: &[EnumVariant]) -> Vec<(SourceSpan, String)> {
    let mut errors: Vec<(SourceSpan, String)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for v in variants {
        let name = v.name.as_str();
        if RESERVED_VARIANTS.contains(&name) {
            errors.push((
                v.span,
                format!(
                    "a variant may not be named `{name}`: `{enum_name}.{name}` is the enum's \
                     own `{name}` (`{enum_name}.all` lists its variants, `{enum_name}.from(name)` \
                     looks one up)"
                ),
            ));
        }
        if !seen.insert(name) {
            errors.push((
                v.span,
                format!("enum `{enum_name}` declares the variant `{name}` twice"),
            ));
        }
        let mut members: HashSet<&str> = HashSet::new();
        let fields = v.fields.iter().map(|f| (f.as_str(), "payload field"));
        let data = v.data.iter().map(|d| (d.key.as_str(), "data key"));
        for (member, what) in fields.chain(data) {
            if member == RESERVED_FIELD {
                errors.push((
                    v.span,
                    format!(
                        "variant `{enum_name}.{name}` may not have a {what} named \
                         `{RESERVED_FIELD}`: `.{RESERVED_FIELD}` is every variant's own name \
                         (\"{name}\" here)"
                    ),
                ));
            }
            if !members.insert(member) {
                errors.push((
                    v.span,
                    format!(
                        "variant `{enum_name}.{name}` declares `{member}` twice: payload \
                         fields and data keys of one variant must be distinct"
                    ),
                ));
            }
        }
    }

    // Every variant declares the same data keys. A key most variants have is
    // *missing* from the ones without it; a key only a few have is *extra*
    // on those.
    let mut keys: Vec<&str> = Vec::new();
    for key in variants.iter().flat_map(|v| &v.data).map(|d| d.key.as_str()) {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for key in keys {
        let has = |v: &EnumVariant| v.data.iter().any(|d| d.key == key);
        let with: Vec<&EnumVariant> = variants.iter().filter(|v| has(v)).collect();
        if with.len() == variants.len() {
            continue;
        }
        if with.len() * 2 >= variants.len() {
            let declared_by = &with[0].name;
            for v in variants.iter().filter(|v| !has(v)) {
                errors.push((
                    v.span,
                    format!(
                        "variant `{enum_name}.{}` is missing the data key `{key}`, which \
                         `{enum_name}.{declared_by}` declares: every variant of an enum declares \
                         the same data keys",
                        v.name
                    ),
                ));
            }
        } else {
            for v in with {
                errors.push((
                    v.span,
                    format!(
                        "variant `{enum_name}.{}` has the extra data key `{key}`, which the \
                         other variants of `{enum_name}` do not declare: every variant of an \
                         enum declares the same data keys",
                        v.name
                    ),
                ));
            }
        }
    }
    errors
}

/// Every `enum` declaration in `stmts` that is *not* at the top level, with
/// the span to blame and the name it tried to declare. Enums are top-level
/// only, for the reason classes are (see `nested_class_decls`): the name is
/// one compile-wide declaration, not a scoped one.
fn nested_enum_decls(stmts: &[Stmt]) -> Vec<(SourceSpan, String)> {
    struct Collect(Vec<(SourceSpan, String)>);
    impl crate::ast::ExprVisitor for Collect {
        fn visit_stmt(&mut self, s: &Stmt) {
            if let StmtKind::EnumDecl { name, .. } = &s.kind {
                self.0.push((s.span, name.clone()));
            }
            crate::ast::walk_stmt(self, s);
        }
    }
    let mut v = Collect(Vec::new());
    for stmt in stmts {
        crate::ast::walk_stmt(&mut v, stmt);
    }
    v.0
}
