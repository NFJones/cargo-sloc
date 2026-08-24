//! Owned, reusable Rust source analysis lowered from one lossless syntax tree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ra_ap_syntax::{
    AstNode, AstToken, SyntaxKind, T, TextRange, algo,
    ast::{self, HasName},
};

use crate::error::AppError;
use crate::model::{CfgOption, ContextKind};
use crate::rust_source::{SemanticContextId, SemanticContextKey};

/// Version of the in-memory analysis representation.
pub(crate) const FILE_ANALYSIS_VERSION: u32 = 1;

/// Syntax-independent analysis for one physical file and Rust edition.
#[derive(Debug)]
pub(crate) struct FileAnalysis {
    pub(crate) version: u32,
    pub(crate) lines: LineIndex,
    tokens: Vec<TokenSpan>,
    has_non_whitespace: Vec<bool>,
    attributes: Vec<AttributeGroup>,
    modules: Vec<ModuleDescriptor>,
}

/// Context-specific results computed once from [`FileAnalysis`].
#[derive(Clone, Debug)]
pub(crate) struct EvaluatedFile {
    pub(crate) lines: Vec<LineProjection>,
    pub(crate) modules: Vec<EvaluatedModule>,
    pub(crate) unknown_cfgs: BTreeSet<String>,
}

/// One external module declaration after cfg/path evaluation.
#[derive(Clone, Debug)]
pub(crate) struct EvaluatedModule {
    pub(crate) name: String,
    pub(crate) inline_components: Vec<String>,
    pub(crate) explicit_path: Option<String>,
}

/// Per-context contribution of one physical line.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LineProjection {
    pub(crate) blank: bool,
    pub(crate) comment: bool,
    pub(crate) code: bool,
}

struct LineMasks {
    active: ContextBits,
    comment: ContextBits,
    code: ContextBits,
}

impl LineMasks {
    fn empty(context_count: usize) -> Self {
        Self {
            active: ContextBits::empty(context_count),
            comment: ContextBits::empty(context_count),
            code: ContextBits::empty(context_count),
        }
    }
}

#[derive(Clone, Debug)]
struct AttributeGroup {
    range: ByteRange,
    effects: Vec<MetaEffect>,
}

#[derive(Clone, Debug)]
struct ModuleDescriptor {
    name: String,
    guards: Vec<Vec<MetaEffect>>,
    inline_ancestors: Vec<InlineAncestor>,
    effects: Vec<MetaEffect>,
}

#[derive(Clone, Debug)]
struct InlineAncestor {
    name: String,
    effects: Vec<MetaEffect>,
}

#[derive(Clone, Debug)]
enum MetaEffect {
    Cfg(CfgExpr),
    CfgAttr {
        predicate: CfgExpr,
        generated: Vec<Self>,
    },
    Path(Result<String, String>),
    HarnessOnly,
    Ignore,
    Invalid(String),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CfgExpr {
    True,
    False,
    Option(CfgOption),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Vec<Self>),
    Invalid(String),
}

#[derive(Clone, Copy, Debug)]
struct TokenSpan {
    range: ByteRange,
    kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TokenKind {
    Comment,
    Code,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

struct ActiveContextSpan {
    range: ByteRange,
    contexts: ContextBits,
}

/// Physical-line boundaries and offset lookup lowered once per file.
#[derive(Debug)]
pub(crate) struct LineIndex {
    pub(crate) ranges: Vec<LineRange>,
    starts: Vec<usize>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LineRange {
    pub(crate) full_start: usize,
    pub(crate) full_end: usize,
}

impl FileAnalysis {
    /// Lowers one parsed file into owned, thread-safe analysis data.
    pub(crate) fn lower(file: &ast::SourceFile, text: &str, path: &Path) -> Result<Self, AppError> {
        let offset = if text.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let lines = LineIndex::new(text);
        let mut tokens = Vec::new();
        let mut has_non_whitespace = vec![false; lines.ranges.len()];
        for token in file
            .syntax()
            .descendants_with_tokens()
            .filter_map(|item| item.into_token())
        {
            if token.kind() == SyntaxKind::WHITESPACE {
                continue;
            }
            let syntax_range = token.text_range();
            let range = ByteRange {
                start: usize::from(syntax_range.start()) + offset,
                end: usize::from(syntax_range.end()) + offset,
            };
            lines.mark_lines(range.start, range.end, |line| {
                has_non_whitespace[line] = true
            });
            tokens.push(TokenSpan {
                range,
                kind: if token.kind() == SyntaxKind::COMMENT {
                    TokenKind::Comment
                } else {
                    TokenKind::Code
                },
            });
        }

        let mut groups = BTreeMap::<(usize, usize), Vec<MetaEffect>>::new();
        for attribute in file.syntax().descendants().filter_map(ast::Attr::cast) {
            let owner = attribute
                .syntax()
                .parent()
                .ok_or_else(|| attribute_error(path, "attribute has no syntax owner"))?;
            let range = governed_range(&owner);
            if let Some(meta) = attribute.meta() {
                groups
                    .entry((
                        usize::from(range.start()) + offset,
                        usize::from(range.end()) + offset,
                    ))
                    .or_default()
                    .push(lower_meta(meta, path));
            }
        }
        let attributes = groups
            .into_iter()
            .map(|((start, end), effects)| AttributeGroup {
                range: ByteRange { start, end },
                effects,
            })
            .collect();

        let modules = file
            .syntax()
            .descendants()
            .filter_map(ast::Module::cast)
            .filter(|module| module.item_list().is_none())
            .map(|module| lower_module(module, path))
            .collect::<Result<Vec<_>, _>>()?;

        crate::metrics::record_file_analysis_lowering();
        Ok(Self {
            version: FILE_ANALYSIS_VERSION,
            lines,
            tokens,
            has_non_whitespace,
            attributes,
            modules,
        })
    }

    /// Evaluates many semantic contexts through one predicate table and token walk.
    pub(crate) fn evaluate_many(
        &self,
        contexts: &[(SemanticContextId, &SemanticContextKey)],
        path: &Path,
    ) -> Result<Vec<(SemanticContextId, EvaluatedFile)>, AppError> {
        if self.version != FILE_ANALYSIS_VERSION {
            return Err(AppError::ReportInvariant(format!(
                "source `{}` has unsupported analysis version {}",
                path.display(),
                self.version
            )));
        }
        let predicates = PredicateTable::new(self, contexts);
        let mut inactive_by_group = self
            .attributes
            .iter()
            .map(|group| (group.range, ContextBits::empty(contexts.len())))
            .collect::<Vec<_>>();
        let mut unknown_by_context = Vec::with_capacity(contexts.len());
        let mut modules_by_context = Vec::with_capacity(contexts.len());

        for (index, (_, context)) in contexts.iter().enumerate() {
            let mut unknown = BTreeSet::new();
            for (group_index, group) in self.attributes.iter().enumerate() {
                let state = evaluate_effects(&group.effects, index, &predicates, false, path)?;
                if !state.active
                    || (state.harness_only
                        && !(context.provenance == ContextKind::Test && context.harness))
                {
                    inactive_by_group[group_index].1.set(index);
                }
                collect_unknown_effects(
                    &group.effects,
                    index,
                    &predicates,
                    context,
                    path,
                    &mut unknown,
                )?;
            }
            unknown_by_context.push(unknown);
            modules_by_context.push(evaluate_modules(
                &self.modules,
                index,
                &predicates,
                context,
                path,
            )?);
        }

        let mut projections = self.project_lines(&inactive_by_group, contexts.len(), path)?;

        crate::metrics::record_file_context_evaluations(contexts.len());
        Ok(contexts
            .iter()
            .enumerate()
            .map(|(index, (id, _))| {
                (
                    *id,
                    EvaluatedFile {
                        lines: std::mem::take(&mut projections[index]),
                        modules: std::mem::take(&mut modules_by_context[index]),
                        unknown_cfgs: std::mem::take(&mut unknown_by_context[index]),
                    },
                )
            })
            .collect())
    }

    fn project_lines(
        &self,
        inactive_by_group: &[(ByteRange, ContextBits)],
        context_count: usize,
        path: &Path,
    ) -> Result<Vec<Vec<LineProjection>>, AppError> {
        let file_end = self.lines.ranges.last().map_or(0, |range| range.full_end);
        let active_spans = active_context_spans(inactive_by_group, context_count, file_end, path)?;
        let mut masks = (0..self.lines.ranges.len())
            .map(|_| LineMasks::empty(context_count))
            .collect::<Vec<_>>();

        for span in &active_spans {
            self.lines
                .mark_lines(span.range.start, span.range.end, |line| {
                    masks[line].active.union(&span.contexts);
                });
        }

        let mut first_active_span = 0;
        for token in &self.tokens {
            while active_spans
                .get(first_active_span)
                .is_some_and(|span| span.range.end <= token.range.start)
            {
                first_active_span += 1;
            }
            for span in active_spans.iter().skip(first_active_span) {
                if span.range.start >= token.range.end {
                    break;
                }
                let start = token.range.start.max(span.range.start);
                let end = token.range.end.min(span.range.end);
                self.lines.mark_lines(start, end, |line| match token.kind {
                    TokenKind::Comment => masks[line].comment.union(&span.contexts),
                    TokenKind::Code => masks[line].code.union(&span.contexts),
                });
            }
        }

        let mut projections =
            vec![vec![LineProjection::default(); self.lines.ranges.len()]; context_count];
        for (line, masks) in masks.iter().enumerate() {
            if !self.has_non_whitespace[line] {
                masks.active.for_each_set(|context| {
                    projections[context][line].blank = true;
                });
            }
            masks.comment.for_each_set(|context| {
                projections[context][line].comment = true;
            });
            masks.code.for_each_set(|context| {
                projections[context][line].code = true;
            });
        }
        Ok(projections)
    }

    fn expressions(&self) -> BTreeSet<CfgExpr> {
        let mut expressions = BTreeSet::new();
        for group in &self.attributes {
            collect_expressions(&group.effects, &mut expressions);
        }
        for module in &self.modules {
            for guard in &module.guards {
                collect_expressions(guard, &mut expressions);
            }
            for ancestor in &module.inline_ancestors {
                collect_expressions(&ancestor.effects, &mut expressions);
            }
            collect_expressions(&module.effects, &mut expressions);
        }
        expressions
    }
}

fn lower_module(module: ast::Module, path: &Path) -> Result<ModuleDescriptor, AppError> {
    let name = module
        .name()
        .map(|name| semantic_identifier(name.text()).to_owned())
        .ok_or_else(|| attribute_error(path, "external module has no name"))?;
    let guards = module
        .syntax()
        .ancestors()
        .filter_map(ast::AnyHasAttrs::cast)
        .map(|owner| lower_effects(&owner, path))
        .collect();
    let mut inline_ancestors = Vec::new();
    let mut current = module.parent();
    while let Some(parent) = current {
        current = parent.parent();
        if parent.item_list().is_some() {
            let name = parent
                .name()
                .map(|name| semantic_identifier(name.text()).to_owned())
                .ok_or_else(|| attribute_error(path, "inline module has no name"))?;
            inline_ancestors.push(InlineAncestor {
                name,
                effects: lower_effects(&parent, path),
            });
        }
    }
    inline_ancestors.reverse();
    Ok(ModuleDescriptor {
        name,
        guards,
        inline_ancestors,
        effects: lower_effects(&module, path),
    })
}

fn lower_effects(owner: &impl ast::HasAttrs, path: &Path) -> Vec<MetaEffect> {
    owner
        .attrs()
        .filter_map(|attribute| attribute.meta())
        .map(|meta| lower_meta(meta, path))
        .collect()
}

fn lower_meta(meta: ast::Meta, path: &Path) -> MetaEffect {
    match meta {
        ast::Meta::CfgMeta(cfg) => cfg.cfg_predicate().map_or_else(
            || MetaEffect::Invalid("cfg attribute has no predicate".to_owned()),
            |predicate| MetaEffect::Cfg(lower_predicate(predicate, path)),
        ),
        ast::Meta::CfgAttrMeta(cfg_attr) => cfg_attr.cfg_predicate().map_or_else(
            || MetaEffect::Invalid("cfg_attr has no predicate".to_owned()),
            |predicate| MetaEffect::CfgAttr {
                predicate: lower_predicate(predicate, path),
                generated: cfg_attr
                    .metas()
                    .map(|meta| lower_meta(meta, path))
                    .collect(),
            },
        ),
        ast::Meta::KeyValueMeta(key_value) => {
            let name = key_value
                .path()
                .and_then(|path| path.as_single_name_ref())
                .map(|name| name.text().to_string());
            if name.as_deref() == Some("path") {
                MetaEffect::Path(meta_string_value(&key_value))
            } else {
                MetaEffect::Ignore
            }
        }
        ast::Meta::PathMeta(path_meta) => {
            let name = path_meta
                .path()
                .and_then(|path| path.as_single_name_ref())
                .map(|name| name.text().to_string());
            if matches!(name.as_deref(), Some("test" | "bench")) {
                MetaEffect::HarnessOnly
            } else {
                MetaEffect::Ignore
            }
        }
        _ => MetaEffect::Ignore,
    }
}

fn lower_predicate(predicate: ast::CfgPredicate, path: &Path) -> CfgExpr {
    match predicate {
        ast::CfgPredicate::CfgAtom(atom) => {
            if atom.true_token().is_some() {
                CfgExpr::True
            } else if atom.false_token().is_some() {
                CfgExpr::False
            } else {
                match cfg_atom_option(atom) {
                    Ok(option) => CfgExpr::Option(option),
                    Err(message) => CfgExpr::Invalid(format!("{}: {message}", path.display())),
                }
            }
        }
        ast::CfgPredicate::CfgComposite(composite) => {
            let Some(keyword) = composite.keyword().map(|token| token.text().to_owned()) else {
                return CfgExpr::Invalid("cfg composite has no operator".to_owned());
            };
            let values = composite
                .cfg_predicates()
                .map(|predicate| lower_predicate(predicate, path))
                .collect();
            match keyword.as_str() {
                "all" => CfgExpr::All(values),
                "any" => CfgExpr::Any(values),
                "not" if values.len() == 1 => CfgExpr::Not(values),
                "not" => CfgExpr::Invalid("cfg not() requires exactly one predicate".to_owned()),
                _ => CfgExpr::Invalid(format!("unsupported cfg operator `{keyword}`")),
            }
        }
    }
}

fn cfg_atom_option(atom: ast::CfgAtom) -> Result<CfgOption, String> {
    let name = atom
        .ident_token()
        .map(|token| semantic_identifier(token.text()).to_owned())
        .ok_or_else(|| "cfg atom has no name".to_owned())?;
    if atom.eq_token().is_some() {
        let token = atom
            .string_token()
            .and_then(ast::String::cast)
            .ok_or_else(|| "cfg name-value atom has no string value".to_owned())?;
        let value = token
            .value()
            .map_err(|error| format!("invalid cfg string value: {error:?}"))?
            .into_owned();
        Ok(CfgOption::KeyValue { name, value })
    } else {
        Ok(CfgOption::Name(name))
    }
}

fn meta_string_value(meta: &ast::KeyValueMeta) -> Result<String, String> {
    let token = meta
        .expr()
        .and_then(|expr| {
            expr.syntax()
                .descendants_with_tokens()
                .filter_map(|element| element.into_token())
                .find_map(ast::String::cast)
        })
        .ok_or_else(|| "path attribute value is not a string literal".to_owned())?;
    token
        .value()
        .map(|value| value.into_owned())
        .map_err(|error| format!("invalid path attribute string: {error:?}"))
}

#[derive(Default)]
struct EffectState {
    active: bool,
    harness_only: bool,
    path: Option<String>,
}

fn evaluate_effects(
    effects: &[MetaEffect],
    context_index: usize,
    predicates: &PredicateTable,
    include_path: bool,
    path: &Path,
) -> Result<EffectState, AppError> {
    let mut state = EffectState {
        active: true,
        harness_only: false,
        path: None,
    };
    apply_effects(
        effects,
        context_index,
        predicates,
        include_path,
        path,
        &mut state,
    )?;
    Ok(state)
}

fn apply_effects(
    effects: &[MetaEffect],
    context_index: usize,
    predicates: &PredicateTable,
    include_path: bool,
    path: &Path,
    state: &mut EffectState,
) -> Result<(), AppError> {
    for effect in effects {
        match effect {
            MetaEffect::Cfg(predicate) => {
                state.active &= predicates.matches(predicate, context_index, path)?;
            }
            MetaEffect::CfgAttr {
                predicate,
                generated,
            } => {
                if predicates.matches(predicate, context_index, path)? {
                    apply_effects(
                        generated,
                        context_index,
                        predicates,
                        include_path,
                        path,
                        state,
                    )?;
                }
            }
            MetaEffect::Path(value) if include_path => {
                let value = value
                    .as_ref()
                    .map_err(|message| attribute_error(path, message))?
                    .clone();
                if state.path.replace(value).is_some() {
                    return Err(attribute_error(
                        path,
                        "module has multiple active path attributes",
                    ));
                }
            }
            MetaEffect::HarnessOnly => state.harness_only = true,
            MetaEffect::Invalid(message) => return Err(attribute_error(path, message)),
            MetaEffect::Path(_) | MetaEffect::Ignore => {}
        }
        if !state.active {
            break;
        }
    }
    Ok(())
}

fn evaluate_modules(
    modules: &[ModuleDescriptor],
    context_index: usize,
    predicates: &PredicateTable,
    context: &SemanticContextKey,
    path: &Path,
) -> Result<Vec<EvaluatedModule>, AppError> {
    let mut evaluated = Vec::new();
    for module in modules {
        let mut active = true;
        for guard in &module.guards {
            let state = evaluate_effects(guard, context_index, predicates, true, path)?;
            if !state.active
                || (state.harness_only
                    && !(context.provenance == ContextKind::Test && context.harness))
            {
                active = false;
                break;
            }
        }
        if !active {
            continue;
        }
        let mut inline_components = Vec::new();
        for ancestor in &module.inline_ancestors {
            let state = evaluate_effects(&ancestor.effects, context_index, predicates, true, path)?;
            inline_components.push(state.path.unwrap_or_else(|| ancestor.name.clone()));
        }
        let state = evaluate_effects(&module.effects, context_index, predicates, true, path)?;
        evaluated.push(EvaluatedModule {
            name: module.name.clone(),
            inline_components,
            explicit_path: state.path,
        });
    }
    Ok(evaluated)
}

fn collect_unknown_effects(
    effects: &[MetaEffect],
    context_index: usize,
    predicates: &PredicateTable,
    context: &SemanticContextKey,
    path: &Path,
    unknown: &mut BTreeSet<String>,
) -> Result<(), AppError> {
    for effect in effects {
        match effect {
            MetaEffect::Cfg(predicate) => collect_unknown_expr(predicate, context, unknown),
            MetaEffect::CfgAttr {
                predicate,
                generated,
            } => {
                collect_unknown_expr(predicate, context, unknown);
                if predicates.matches(predicate, context_index, path)? {
                    collect_unknown_effects(
                        generated,
                        context_index,
                        predicates,
                        context,
                        path,
                        unknown,
                    )?;
                }
            }
            MetaEffect::Invalid(message) => return Err(attribute_error(path, message)),
            MetaEffect::Path(_) | MetaEffect::HarnessOnly | MetaEffect::Ignore => {}
        }
    }
    Ok(())
}

fn collect_unknown_expr(
    expression: &CfgExpr,
    context: &SemanticContextKey,
    unknown: &mut BTreeSet<String>,
) {
    match expression {
        CfgExpr::Option(option) => {
            let recognized = match option {
                CfgOption::Name(name) => context.recognized_cfg_names.contains(name),
                CfgOption::KeyValue { name, value } if name == "feature" => {
                    context.recognized_features.contains(value)
                }
                CfgOption::KeyValue { name, .. } => context.recognized_cfg_names.contains(name),
            };
            if !recognized {
                unknown.insert(format_cfg_option(option));
            }
        }
        CfgExpr::All(values) | CfgExpr::Any(values) | CfgExpr::Not(values) => {
            for value in values {
                collect_unknown_expr(value, context, unknown);
            }
        }
        CfgExpr::True | CfgExpr::False | CfgExpr::Invalid(_) => {}
    }
}

fn collect_expressions(effects: &[MetaEffect], expressions: &mut BTreeSet<CfgExpr>) {
    for effect in effects {
        match effect {
            MetaEffect::Cfg(predicate) => {
                if !matches!(predicate, CfgExpr::Invalid(_)) {
                    expressions.insert(predicate.clone());
                }
            }
            MetaEffect::CfgAttr {
                predicate,
                generated,
            } => {
                if !matches!(predicate, CfgExpr::Invalid(_)) {
                    expressions.insert(predicate.clone());
                }
                collect_expressions(generated, expressions);
            }
            MetaEffect::Path(_)
            | MetaEffect::HarnessOnly
            | MetaEffect::Ignore
            | MetaEffect::Invalid(_) => {}
        }
    }
}

struct PredicateTable {
    values: BTreeMap<CfgExpr, ContextBits>,
}

impl PredicateTable {
    fn new(analysis: &FileAnalysis, contexts: &[(SemanticContextId, &SemanticContextKey)]) -> Self {
        let values = analysis
            .expressions()
            .into_iter()
            .map(|expression| {
                let bits = evaluate_expression_bits(&expression, contexts);
                (expression, bits)
            })
            .collect();
        Self { values }
    }

    fn matches(
        &self,
        expression: &CfgExpr,
        context_index: usize,
        path: &Path,
    ) -> Result<bool, AppError> {
        if let CfgExpr::Invalid(message) = expression {
            return Err(attribute_error(path, message));
        }
        self.values
            .get(expression)
            .map(|bits| bits.contains(context_index))
            .ok_or_else(|| {
                AppError::ReportInvariant(format!(
                    "source `{}` references an uninterned cfg expression",
                    path.display()
                ))
            })
    }
}

#[derive(Clone)]
struct ContextBits {
    words: Vec<u64>,
    len: usize,
}

impl ContextBits {
    fn empty(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    fn full(len: usize) -> Self {
        let mut bits = Self {
            words: vec![u64::MAX; len.div_ceil(64)],
            len,
        };
        bits.mask_tail();
        bits
    }

    fn set(&mut self, index: usize) {
        self.words[index / 64] |= 1_u64 << (index % 64);
    }

    fn clear(&mut self, index: usize) {
        self.words[index / 64] &= !(1_u64 << (index % 64));
    }

    fn contains(&self, index: usize) -> bool {
        self.words
            .get(index / 64)
            .is_some_and(|word| word & (1_u64 << (index % 64)) != 0)
    }

    fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    fn for_each_set(&self, mut visit: impl FnMut(usize)) {
        for (word_index, word) in self.words.iter().copied().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                let bit = remaining.trailing_zeros() as usize;
                let index = word_index * 64 + bit;
                if index < self.len {
                    visit(index);
                }
                remaining &= remaining - 1;
            }
        }
    }

    fn intersect(&mut self, other: &Self) {
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left &= right;
        }
    }

    fn union(&mut self, other: &Self) {
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left |= right;
        }
    }

    fn invert(&mut self) {
        for word in &mut self.words {
            *word = !*word;
        }
        self.mask_tail();
    }

    fn mask_tail(&mut self) {
        let remainder = self.len % 64;
        if remainder != 0
            && let Some(last) = self.words.last_mut()
        {
            *last &= (1_u64 << remainder) - 1;
        }
    }
}

fn active_context_spans(
    inactive_by_group: &[(ByteRange, ContextBits)],
    context_count: usize,
    file_end: usize,
    path: &Path,
) -> Result<Vec<ActiveContextSpan>, AppError> {
    let mut events = BTreeMap::<usize, Vec<(bool, &ContextBits)>>::new();
    for (range, contexts) in inactive_by_group {
        if range.start > range.end || range.end > file_end {
            return Err(AppError::ReportInvariant(format!(
                "source `{}` has an invalid cfg-governed byte range {}..{} for length {file_end}",
                path.display(),
                range.start,
                range.end
            )));
        }
        if range.start == range.end || contexts.is_empty() {
            continue;
        }
        events
            .entry(range.start)
            .or_default()
            .push((true, contexts));
        events.entry(range.end).or_default().push((false, contexts));
    }

    let mut depths = vec![0_u32; context_count];
    let mut active = ContextBits::full(context_count);
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (offset, changes) in events {
        if cursor < offset && !active.is_empty() {
            spans.push(ActiveContextSpan {
                range: ByteRange {
                    start: cursor,
                    end: offset,
                },
                contexts: active.clone(),
            });
        }
        for (starting, contexts) in changes {
            let mut update_error = None;
            contexts.for_each_set(|context| {
                if update_error.is_some() {
                    return;
                }
                if starting {
                    match depths[context].checked_add(1) {
                        Some(depth) => {
                            depths[context] = depth;
                            if depth == 1 {
                                active.clear(context);
                            }
                        }
                        None => update_error = Some("inactive cfg range depth overflowed"),
                    }
                } else if depths[context] == 0 {
                    update_error = Some("inactive cfg range ended before it started");
                } else {
                    depths[context] -= 1;
                    if depths[context] == 0 {
                        active.set(context);
                    }
                }
            });
            if let Some(message) = update_error {
                return Err(AppError::ReportInvariant(format!(
                    "source `{}` {message}",
                    path.display()
                )));
            }
        }
        cursor = offset;
    }
    if depths.iter().any(|depth| *depth != 0) {
        return Err(AppError::ReportInvariant(format!(
            "source `{}` has an unterminated inactive cfg range",
            path.display()
        )));
    }
    if cursor < file_end && !active.is_empty() {
        spans.push(ActiveContextSpan {
            range: ByteRange {
                start: cursor,
                end: file_end,
            },
            contexts: active,
        });
    }
    Ok(spans)
}

fn evaluate_expression_bits(
    expression: &CfgExpr,
    contexts: &[(SemanticContextId, &SemanticContextKey)],
) -> ContextBits {
    match expression {
        CfgExpr::True => ContextBits::full(contexts.len()),
        CfgExpr::False | CfgExpr::Invalid(_) => ContextBits::empty(contexts.len()),
        CfgExpr::Option(option) => {
            let mut bits = ContextBits::empty(contexts.len());
            for (index, (_, context)) in contexts.iter().enumerate() {
                if context.cfg_options.contains(option) {
                    bits.set(index);
                }
            }
            bits
        }
        CfgExpr::All(values) => {
            let mut bits = ContextBits::full(contexts.len());
            for value in values {
                bits.intersect(&evaluate_expression_bits(value, contexts));
            }
            bits
        }
        CfgExpr::Any(values) => {
            let mut bits = ContextBits::empty(contexts.len());
            for value in values {
                bits.union(&evaluate_expression_bits(value, contexts));
            }
            bits
        }
        CfgExpr::Not(values) if values.len() == 1 => {
            let mut bits = evaluate_expression_bits(&values[0], contexts);
            bits.invert();
            bits
        }
        CfgExpr::Not(_) => ContextBits::empty(contexts.len()),
    }
}

impl LineIndex {
    fn new(text: &str) -> Self {
        if text.is_empty() {
            return Self {
                ranges: Vec::new(),
                starts: Vec::new(),
            };
        }
        let mut ranges = Vec::new();
        let mut start = 0;
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                ranges.push(LineRange {
                    full_start: start,
                    full_end: index + 1,
                });
                start = index + 1;
            }
        }
        if start < text.len() {
            ranges.push(LineRange {
                full_start: start,
                full_end: text.len(),
            });
        }
        let starts = ranges.iter().map(|range| range.full_start).collect();
        Self { ranges, starts }
    }

    fn mark_lines(&self, start: usize, end: usize, mut mark: impl FnMut(usize)) {
        if start >= end || self.ranges.is_empty() {
            return;
        }
        let first = self.line_for_offset(start);
        let last = self.line_for_offset(end.saturating_sub(1));
        for line in first..=last {
            mark(line);
        }
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
            .min(self.ranges.len() - 1)
    }
}

fn governed_range(owner: &ra_ap_syntax::SyntaxNode) -> TextRange {
    let mut range = owner
        .parent()
        .filter(|parent| parent.kind() == SyntaxKind::EXPR_STMT)
        .map_or_else(|| owner.text_range(), |statement| statement.text_range());
    if let Some(comma) =
        algo::next_non_trivia_token(owner.clone()).filter(|token| token.kind() == T![,])
    {
        range = TextRange::new(range.start(), comma.text_range().end());
    }
    range
}

fn format_cfg_option(option: &CfgOption) -> String {
    match option {
        CfgOption::Name(name) => name.clone(),
        CfgOption::KeyValue { name, value } => format!(
            "{name} = {}",
            serde_json::to_string(value).expect("String JSON cannot fail")
        ),
    }
}

fn semantic_identifier(identifier: &str) -> &str {
    identifier.strip_prefix("r#").unwrap_or(identifier)
}

fn attribute_error(path: &Path, message: &str) -> AppError {
    AppError::ModuleAttribute {
        path: PathBuf::from(path),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_bits_preserve_values_across_word_boundaries() {
        let mut bits = ContextBits::empty(130);
        for index in [0, 63, 64, 65, 127, 128, 129] {
            bits.set(index);
        }

        for index in 0..130 {
            assert_eq!(
                bits.contains(index),
                matches!(index, 0 | 63 | 64 | 65 | 127 | 128 | 129)
            );
        }

        bits.invert();
        for index in 0..130 {
            assert_eq!(
                bits.contains(index),
                !matches!(index, 0 | 63 | 64 | 65 | 127 | 128 | 129)
            );
        }
    }

    #[test]
    fn context_bit_operations_mask_partial_tail_words() {
        let mut all = ContextBits::full(65);
        let mut selected = ContextBits::empty(65);
        selected.set(64);
        all.intersect(&selected);
        assert!(all.contains(64));
        assert!((0..64).all(|index| !all.contains(index)));

        let mut first = ContextBits::empty(65);
        first.set(0);
        all.union(&first);
        assert!(all.contains(0));
        assert!(all.contains(64));
    }

    #[test]
    fn active_context_spans_preserve_overlapping_inactive_depths() {
        let mut outer = ContextBits::empty(3);
        outer.set(0);
        outer.set(1);
        let mut inner = ContextBits::empty(3);
        inner.set(1);
        inner.set(2);

        let spans = active_context_spans(
            &[
                (ByteRange { start: 2, end: 8 }, outer),
                (ByteRange { start: 4, end: 6 }, inner),
            ],
            3,
            10,
            Path::new("overlap.rs"),
        )
        .expect("sweep overlapping inactive ranges");
        let actual = spans
            .iter()
            .map(|span| {
                let mut contexts = Vec::new();
                span.contexts.for_each_set(|context| contexts.push(context));
                ((span.range.start, span.range.end), contexts)
            })
            .collect::<Vec<_>>();

        assert_eq!(
            actual,
            vec![
                ((0, 2), vec![0, 1, 2]),
                ((2, 4), vec![2]),
                ((6, 8), vec![2]),
                ((8, 10), vec![0, 1, 2]),
            ]
        );
    }
}
