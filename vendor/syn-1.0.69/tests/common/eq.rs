extern crate rustc_ast;
extern crate rustc_data_structures;
extern crate rustc_span;

use rustc_ast::ast::{
    AngleBracketedArg, AngleBracketedArgs, AnonConst, Arm, AssocItemKind, AssocTyConstraint,
    AssocTyConstraintKind, Async, AttrId, AttrItem, AttrKind, AttrStyle, Attribute, BareFnTy,
    BinOpKind, BindingMode, Block, BlockCheckMode, BorrowKind, CaptureBy, Const, Crate, CrateSugar,
    Defaultness, EnumDef, Expr, ExprField, ExprKind, Extern, FieldDef, FloatTy, FnDecl, FnHeader,
    FnKind, FnRetTy, FnSig, ForeignItemKind, ForeignMod, GenericArg, GenericArgs, GenericBound,
    GenericParam, GenericParamKind, Generics, GlobalAsm, ImplKind, ImplPolarity, Inline, InlineAsm,
    InlineAsmOperand, InlineAsmOptions, InlineAsmRegOrRegClass, InlineAsmTemplatePiece, IntTy,
    IsAuto, Item, ItemKind, Label, Lifetime, Lit, LitFloatType, LitIntType, LitKind,
    LlvmAsmDialect, LlvmInlineAsm, LlvmInlineAsmOutput, Local, MacArgs, MacCall, MacCallStmt,
    MacDelimiter, MacStmtStyle, MacroDef, ModKind, Movability, MutTy, Mutability, NodeId, Param,
    ParenthesizedArgs, Pat, PatField, PatKind, Path, PathSegment, PolyTraitRef, QSelf, RangeEnd,
    RangeLimits, RangeSyntax, Stmt, StmtKind, StrLit, StrStyle, StructExpr, StructRest,
    TraitBoundModifier, TraitKind, TraitObjectSyntax, TraitRef, Ty, TyAliasKind, TyKind, UintTy,
    UnOp, Unsafe, UnsafeSource, UseTree, UseTreeKind, Variant, VariantData, Visibility,
    VisibilityKind, WhereBoundPredicate, WhereClause, WhereEqPredicate, WherePredicate,
    WhereRegionPredicate,
};
use rustc_ast::ptr::P;
use rustc_ast::token::{self, CommentKind, DelimToken, Nonterminal, Token, TokenKind};
use rustc_ast::tokenstream::{DelimSpan, LazyTokenStream, TokenStream, TokenTree};
use rustc_data_structures::sync::Lrc;
use rustc_data_structures::thin_vec::ThinVec;
use rustc_span::source_map::Spanned;
use rustc_span::symbol::{sym, Ident};
use rustc_span::{Span, Symbol, SyntaxContext, DUMMY_SP};

pub trait SpanlessEq {
    fn eq(&self, other: &Self) -> bool;
}

impl<T: SpanlessEq> SpanlessEq for Box<T> {
    fn eq(&self, other: &Self) -> bool {
        SpanlessEq::eq(&**self, &**other)
    }
}

impl<T: SpanlessEq> SpanlessEq for P<T> {
    fn eq(&self, other: &Self) -> bool {
        SpanlessEq::eq(&**self, &**other)
    }
}

impl<T: ?Sized + SpanlessEq> SpanlessEq for Lrc<T> {
    fn eq(&self, other: &Self) -> bool {
        SpanlessEq::eq(&**self, &**other)
    }
}

impl<T: SpanlessEq> SpanlessEq for Option<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (None, None) => true,
            (Some(this), Some(other)) => SpanlessEq::eq(this, other),
            _ => false,
        }
    }
}

impl<T: SpanlessEq> SpanlessEq for [T] {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| SpanlessEq::eq(a, b))
    }
}

impl<T: SpanlessEq> SpanlessEq for Vec<T> {
    fn eq(&self, other: &Self) -> bool {
        <[T] as SpanlessEq>::eq(self, other)
    }
}

impl<T: SpanlessEq> SpanlessEq for ThinVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .zip(other.iter())
                .all(|(a, b)| SpanlessEq::eq(a, b))
    }
}

impl<T: SpanlessEq> SpanlessEq for Spanned<T> {
    fn eq(&self, other: &Self) -> bool {
        SpanlessEq::eq(&self.node, &other.node)
    }
}

impl<A: SpanlessEq, B: SpanlessEq> SpanlessEq for (A, B) {
    fn eq(&self, other: &Self) -> bool {
        SpanlessEq::eq(&self.0, &other.0) && SpanlessEq::eq(&self.1, &other.1)
    }
}

macro_rules! spanless_eq_true {
    ($name:ty) => {
        impl SpanlessEq for $name {
            fn eq(&self, _other: &Self) -> bool {
                true
            }
        }
    };
}

spanless_eq_true!(Span);
spanless_eq_true!(DelimSpan);
spanless_eq_true!(AttrId);
spanless_eq_true!(NodeId);
spanless_eq_true!(SyntaxContext);

macro_rules! spanless_eq_partial_eq {
    ($name:ty) => {
        impl SpanlessEq for $name {
            fn eq(&self, other: &Self) -> bool {
                PartialEq::eq(self, other)
            }
        }
    };
}

spanless_eq_partial_eq!(bool);
spanless_eq_partial_eq!(u8);
spanless_eq_partial_eq!(u16);
spanless_eq_partial_eq!(u128);
spanless_eq_partial_eq!(usize);
spanless_eq_partial_eq!(char);
spanless_eq_partial_eq!(String);
spanless_eq_partial_eq!(Symbol);
spanless_eq_partial_eq!(CommentKind);
spanless_eq_partial_eq!(DelimToken);
spanless_eq_partial_eq!(InlineAsmOptions);
spanless_eq_partial_eq!(token::LitKind);

macro_rules! spanless_eq_struct {
    {
        $($name:ident)::+ $(<$param:ident>)?
        $([$field:tt $this:ident $other:ident])*
        $(![$ignore:tt])*;
    } => {
        impl $(<$param: SpanlessEq>)* SpanlessEq for $($name)::+ $(<$param>)* {
            fn eq(&self, other: &Self) -> bool {
                let $($name)::+ { $($field: $this,)* $($ignore: _,)* } = self;
                let $($name)::+ { $($field: $other,)* $($ignore: _,)* } = other;
                true $(&& SpanlessEq::eq($this, $other))*
            }
        }
    };

    {
        $($name:ident)::+ $(<$param:ident>)?
        $([$field:tt $this:ident $other:ident])*
        $(![$ignore:tt])*;
        !$next:tt
        $($rest:tt)*
    } => {
        spanless_eq_struct! {
            $($name)::+ $(<$param>)*
            $([$field $this $other])*
            $(![$ignore])*
            ![$next];
            $($rest)*
        }
    };

    {
        $($name:ident)::+ $(<$param:ident>)?
        $([$field:tt $this:ident $other:ident])*
        $(![$ignore:tt])*;
        $next:tt
        $($rest:tt)*
    } => {
        spanless_eq_struct! {
            $($name)::+ $(<$param>)*
            $([$field $this $other])*
            [$next this other]
            $(![$ignore])*;
            $($rest)*
        }
    };
}

macro_rules! spanless_eq_enum {
    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $([$field:tt $this:ident $other:ident])* $(![$ignore:tt])*])*
    } => {
        impl SpanlessEq for $($name)::+ {
            fn eq(&self, other: &Self) -> bool {
                match self {
                    $(
                        $($variant)::+ { .. } => {}
                    )*
                }
                #[allow(unreachable_patterns)]
                match (self, other) {
                    $(
                        (
                            $($variant)::+ { $($field: $this,)* $($ignore: _,)* },
                            $($variant)::+ { $($field: $other,)* $($ignore: _,)* },
                        ) => {
                            true $(&& SpanlessEq::eq($this, $other))*
                        }
                    )*
                    _ => false,
                }
            }
        }
    };

    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $($fields:tt)*])*
        $next:ident [$([$($named:tt)*])* $(![$ignore:tt])*] (!$i:tt $($field:tt)*)
        $($rest:tt)*
    } => {
        spanless_eq_enum! {
            $($name)::+;
            $([$($variant)::+; $($fields)*])*
            $next [$([$($named)*])* $(![$ignore])* ![$i]] ($($field)*)
            $($rest)*
        }
    };

    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $($fields:tt)*])*
        $next:ident [$([$($named:tt)*])* $(![$ignore:tt])*] ($i:tt $($field:tt)*)
        $($rest:tt)*
    } => {
        spanless_eq_enum! {
            $($name)::+;
            $([$($variant)::+; $($fields)*])*
            $next [$([$($named)*])* [$i this other] $(![$ignore])*] ($($field)*)
            $($rest)*
        }
    };

    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $($fields:tt)*])*
        $next:ident [$($named:tt)*] ()
        $($rest:tt)*
    } => {
        spanless_eq_enum! {
            $($name)::+;
            $([$($variant)::+; $($fields)*])*
            [$($name)::+::$next; $($named)*]
            $($rest)*
        }
    };

    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $($fields:tt)*])*
        $next:ident ($($field:tt)*)
        $($rest:tt)*
    } => {
        spanless_eq_enum! {
            $($name)::+;
            $([$($variant)::+; $($fields)*])*
            $next [] ($($field)*)
            $($rest)*
        }
    };

    {
        $($name:ident)::+;
        $([$($variant:ident)::+; $($fields:tt)*])*
        $next:ident
        $($rest:tt)*
    } => {
        spanless_eq_enum! {
            $($name)::+;
            $([$($variant)::+; $($fields)*])*
            [$($name)::+::$next;]
            $($rest)*
        }
    };
}

spanless_eq_struct!(AngleBracketedArgs; span args);
spanless_eq_struct!(AnonConst; id value);
spanless_eq_struct!(Arm; attrs pat guard body span id is_placeholder);
spanless_eq_struct!(AssocTyConstraint; id ident gen_args kind span);
spanless_eq_struct!(AttrItem; path args tokens);
spanless_eq_struct!(Attribute; kind id style span);
spanless_eq_struct!(BareFnTy; unsafety ext generic_params decl);
spanless_eq_struct!(Block; stmts id rules span tokens);
spanless_eq_struct!(Crate; attrs items span proc_macros);
spanless_eq_struct!(EnumDef; variants);
spanless_eq_struct!(Expr; id kind span attrs !tokens);
spanless_eq_struct!(ExprField; attrs id span ident expr is_shorthand is_placeholder);
spanless_eq_struct!(FieldDef; attrs id span vis ident ty is_placeholder);
spanless_eq_struct!(FnDecl; inputs output);
spanless_eq_struct!(FnHeader; constness asyncness unsafety ext);
spanless_eq_struct!(FnKind; 0 1 2 3);
spanless_eq_struct!(FnSig; header decl span);
spanless_eq_struct!(ForeignMod; unsafety abi items);
spanless_eq_struct!(GenericParam; id ident attrs bounds is_placeholder kind);
spanless_eq_struct!(Generics; params where_clause span);
spanless_eq_struct!(GlobalAsm; asm);
spanless_eq_struct!(ImplKind; unsafety polarity defaultness constness generics of_trait self_ty items);
spanless_eq_struct!(InlineAsm; template operands options line_spans);
spanless_eq_struct!(Item<K>; attrs id span vis ident kind !tokens);
spanless_eq_struct!(Label; ident);
spanless_eq_struct!(Lifetime; id ident);
spanless_eq_struct!(Lit; token kind span);
spanless_eq_struct!(LlvmInlineAsm; asm asm_str_style outputs inputs clobbers volatile alignstack dialect);
spanless_eq_struct!(LlvmInlineAsmOutput; constraint expr is_rw is_indirect);
spanless_eq_struct!(Local; pat ty init id span attrs !tokens);
spanless_eq_struct!(MacCall; path args prior_type_ascription);
spanless_eq_struct!(MacCallStmt; mac style attrs tokens);
spanless_eq_struct!(MacroDef; body macro_rules);
spanless_eq_struct!(MutTy; ty mutbl);
spanless_eq_struct!(ParenthesizedArgs; span inputs inputs_span output);
spanless_eq_struct!(Pat; id kind span tokens);
spanless_eq_struct!(PatField; ident pat is_shorthand attrs id span is_placeholder);
spanless_eq_struct!(Path; span segments tokens);
spanless_eq_struct!(PathSegment; ident id args);
spanless_eq_struct!(PolyTraitRef; bound_generic_params trait_ref span);
spanless_eq_struct!(QSelf; ty path_span position);
spanless_eq_struct!(Stmt; id kind span);
spanless_eq_struct!(StrLit; style symbol suffix span symbol_unescaped);
spanless_eq_struct!(StructExpr; path fields rest);
spanless_eq_struct!(Token; kind span);
spanless_eq_struct!(TraitKind; 0 1 2 3 4);
spanless_eq_struct!(TraitRef; path ref_id);
spanless_eq_struct!(Ty; id kind span tokens);
spanless_eq_struct!(TyAliasKind; 0 1 2 3);
spanless_eq_struct!(UseTree; prefix kind span);
spanless_eq_struct!(Variant; attrs id span !vis ident data disr_expr is_placeholder);
spanless_eq_struct!(Visibility; kind span tokens);
spanless_eq_struct!(WhereBoundPredicate; span bound_generic_params bounded_ty bounds);
spanless_eq_struct!(WhereClause; has_where_token predicates span);
spanless_eq_struct!(WhereEqPredicate; id span lhs_ty rhs_ty);
spanless_eq_struct!(WhereRegionPredicate; span lifetime bounds);
spanless_eq_struct!(token::Lit; kind symbol suffix);
spanless_eq_enum!(AngleBracketedArg; Arg(0) Constraint(0));
spanless_eq_enum!(AssocItemKind; Const(0 1 2) Fn(0) TyAlias(0) MacCall(0));
spanless_eq_enum!(AssocTyConstraintKind; Equality(ty) Bound(bounds));
spanless_eq_enum!(Async; Yes(span closure_id return_impl_trait_id) No);
spanless_eq_enum!(AttrStyle; Outer Inner);
spanless_eq_enum!(BinOpKind; Add Sub Mul Div Rem And Or BitXor BitAnd BitOr Shl Shr Eq Lt Le Ne Ge Gt);
spanless_eq_enum!(BindingMode; ByRef(0) ByValue(0));
spanless_eq_enum!(BlockCheckMode; Default Unsafe(0));
spanless_eq_enum!(BorrowKind; Ref Raw);
spanless_eq_enum!(CaptureBy; Value Ref);
spanless_eq_enum!(Const; Yes(0) No);
spanless_eq_enum!(CrateSugar; PubCrate JustCrate);
spanless_eq_enum!(Defaultness; Default(0) Final);
spanless_eq_enum!(Extern; None Implicit Explicit(0));
spanless_eq_enum!(FloatTy; F32 F64);
spanless_eq_enum!(FnRetTy; Default(0) Ty(0));
spanless_eq_enum!(ForeignItemKind; Static(0 1 2) Fn(0) TyAlias(0) MacCall(0));
spanless_eq_enum!(GenericArg; Lifetime(0) Type(0) Const(0));
spanless_eq_enum!(GenericArgs; AngleBracketed(0) Parenthesized(0));
spanless_eq_enum!(GenericBound; Trait(0 1) Outlives(0));
spanless_eq_enum!(GenericParamKind; Lifetime Type(default) Const(ty kw_span default));
spanless_eq_enum!(ImplPolarity; Positive Negative(0));
spanless_eq_enum!(Inline; Yes No);
spanless_eq_enum!(InlineAsmRegOrRegClass; Reg(0) RegClass(0));
spanless_eq_enum!(InlineAsmTemplatePiece; String(0) Placeholder(operand_idx modifier span));
spanless_eq_enum!(IntTy; Isize I8 I16 I32 I64 I128);
spanless_eq_enum!(IsAuto; Yes No);
spanless_eq_enum!(LitFloatType; Suffixed(0) Unsuffixed);
spanless_eq_enum!(LitIntType; Signed(0) Unsigned(0) Unsuffixed);
spanless_eq_enum!(LlvmAsmDialect; Att Intel);
spanless_eq_enum!(MacArgs; Empty Delimited(0 1 2) Eq(0 1));
spanless_eq_enum!(MacDelimiter; Parenthesis Bracket Brace);
spanless_eq_enum!(MacStmtStyle; Semicolon Braces NoBraces);
spanless_eq_enum!(ModKind; Loaded(0 1 2) Unloaded);
spanless_eq_enum!(Movability; Static Movable);
spanless_eq_enum!(Mutability; Mut Not);
spanless_eq_enum!(RangeEnd; Included(0) Excluded);
spanless_eq_enum!(RangeLimits; HalfOpen Closed);
spanless_eq_enum!(StmtKind; Local(0) Item(0) Expr(0) Semi(0) Empty MacCall(0));
spanless_eq_enum!(StrStyle; Cooked Raw(0));
spanless_eq_enum!(StructRest; Base(0) Rest(0) None);
spanless_eq_enum!(TokenTree; Token(0) Delimited(0 1 2));
spanless_eq_enum!(TraitBoundModifier; None Maybe MaybeConst MaybeConstMaybe);
spanless_eq_enum!(TraitObjectSyntax; Dyn None);
spanless_eq_enum!(UintTy; Usize U8 U16 U32 U64 U128);
spanless_eq_enum!(UnOp; Deref Not Neg);
spanless_eq_enum!(Unsafe; Yes(0) No);
spanless_eq_enum!(UnsafeSource; CompilerGenerated UserProvided);
spanless_eq_enum!(UseTreeKind; Simple(0 1 2) Nested(0) Glob);
spanless_eq_enum!(VariantData; Struct(0 1) Tuple(0 1) Unit(0));
spanless_eq_enum!(VisibilityKind; Public Crate(0) Restricted(path id) Inherited);
spanless_eq_enum!(WherePredicate; BoundPredicate(0) RegionPredicate(0) EqPredicate(0));
spanless_eq_enum!(ExprKind; Box(0) Array(0) ConstBlock(0) Call(0 1)
    MethodCall(0 1 2) Tup(0) Binary(0 1 2) Unary(0 1) Lit(0) Cast(0 1) Type(0 1)
    Let(0 1) If(0 1 2) While(0 1 2) ForLoop(0 1 2 3) Loop(0 1) Match(0 1)
    Closure(0 1 2 3 4 5) Block(0 1) Async(0 1 2) Await(0) TryBlock(0)
    Assign(0 1 2) AssignOp(0 1 2) Field(0 1) Index(0 1) Underscore Range(0 1 2)
    Path(0 1) AddrOf(0 1 2) Break(0 1) Continue(0) Ret(0) InlineAsm(0)
    LlvmInlineAsm(0) MacCall(0) Struct(0) Repeat(0 1) Paren(0) Try(0) Yield(0)
    Err);
spanless_eq_enum!(InlineAsmOperand; In(reg expr) Out(reg late expr)
    InOut(reg late expr) SplitInOut(reg late in_expr out_expr) Const(anon_const)
    Sym(expr));
spanless_eq_enum!(ItemKind; ExternCrate(0) Use(0) Static(0 1 2) Const(0 1 2)
    Fn(0) Mod(0 1) ForeignMod(0) GlobalAsm(0) TyAlias(0) Enum(0 1) Struct(0 1)
    Union(0 1) Trait(0) TraitAlias(0 1) Impl(0) MacCall(0) MacroDef(0));
spanless_eq_enum!(LitKind; Str(0 1) ByteStr(0) Byte(0) Char(0) Int(0 1)
    Float(0 1) Bool(0) Err(0));
spanless_eq_enum!(PatKind; Wild Ident(0 1 2) Struct(0 1 2) TupleStruct(0 1)
    Or(0) Path(0 1) Tuple(0) Box(0) Ref(0 1) Lit(0) Range(0 1 2) Slice(0) Rest
    Paren(0) MacCall(0));
spanless_eq_enum!(TyKind; Slice(0) Array(0 1) Ptr(0) Rptr(0 1) BareFn(0) Never
    Tup(0) Path(0 1) TraitObject(0 1) ImplTrait(0 1) Paren(0) Typeof(0) Infer
    ImplicitSelf MacCall(0) Err CVarArgs);

impl SpanlessEq for Ident {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl SpanlessEq for RangeSyntax {
    fn eq(&self, _other: &Self) -> bool {
        match self {
            RangeSyntax::DotDotDot | RangeSyntax::DotDotEq => true,
        }
    }
}

impl SpanlessEq for Param {
    fn eq(&self, other: &Self) -> bool {
        let Param {
            attrs,
            ty,
            pat,
            id,
            span: _,
            is_placeholder,
        } = self;
        let Param {
            attrs: attrs2,
            ty: ty2,
            pat: pat2,
            id: id2,
            span: _,
            is_placeholder: is_placeholder2,
        } = other;
        SpanlessEq::eq(id, id2)
            && SpanlessEq::eq(is_placeholder, is_placeholder2)
            && (matches!(ty.kind, TyKind::Err)
                || matches!(ty2.kind, TyKind::Err)
                || SpanlessEq::eq(attrs, attrs2)
                    && SpanlessEq::eq(ty, ty2)
                    && SpanlessEq::eq(pat, pat2))
    }
}

impl SpanlessEq for TokenKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TokenKind::Literal(this), TokenKind::Literal(other)) => SpanlessEq::eq(this, other),
            (TokenKind::DotDotEq, _) | (TokenKind::DotDotDot, _) => match other {
                TokenKind::DotDotEq | TokenKind::DotDotDot => true,
                _ => false,
            },
            (TokenKind::Interpolated(this), TokenKind::Interpolated(other)) => {
                match (this.as_ref(), other.as_ref()) {
                    (Nonterminal::NtExpr(this), Nonterminal::NtExpr(other)) => {
                        SpanlessEq::eq(this, other)
                    }
                    _ => this == other,
                }
            }
            _ => self == other,
        }
    }
}

impl SpanlessEq for TokenStream {
    fn eq(&self, other: &Self) -> bool {
        let mut this_trees = self.trees();
        let mut other_trees = other.trees();
        loop {
            let this = match this_trees.next() {
                None => return other_trees.next().is_none(),
                Some(tree) => tree,
            };
            let other = match other_trees.next() {
                None => return false,
                Some(tree) => tree,
            };
            if SpanlessEq::eq(&this, &other) {
                continue;
            }
            if let (TokenTree::Token(this), TokenTree::Token(other)) = (this, other) {
                if match (&this.kind, &other.kind) {
                    (TokenKind::Literal(this), TokenKind::Literal(other)) => {
                        SpanlessEq::eq(this, other)
                    }
                    (TokenKind::DocComment(_kind, style, symbol), TokenKind::Pound) => {
                        doc_comment(*style, *symbol, &mut other_trees)
                    }
                    (TokenKind::Pound, TokenKind::DocComment(_kind, style, symbol)) => {
                        doc_comment(*style, *symbol, &mut this_trees)
                    }
                    _ => false,
                } {
                    continue;
                }
            }
            return false;
        }
    }
}

fn doc_comment<'a>(
    style: AttrStyle,
    unescaped: Symbol,
    trees: &mut impl Iterator<Item = TokenTree>,
) -> bool {
    if match style {
        AttrStyle::Outer => false,
        AttrStyle::Inner => true,
    } {
        match trees.next() {
            Some(TokenTree::Token(Token {
                kind: TokenKind::Not,
                span: _,
            })) => {}
            _ => return false,
        }
    }
    let stream = match trees.next() {
        Some(TokenTree::Delimited(_span, DelimToken::Bracket, stream)) => stream,
        _ => return false,
    };
    let mut trees = stream.trees();
    match trees.next() {
        Some(TokenTree::Token(Token {
            kind: TokenKind::Ident(symbol, false),
            span: _,
        })) if symbol == sym::doc => {}
        _ => return false,
    }
    match trees.next() {
        Some(TokenTree::Token(Token {
            kind: TokenKind::Eq,
            span: _,
        })) => {}
        _ => return false,
    }
    match trees.next() {
        Some(TokenTree::Token(token)) => {
            is_escaped_literal(&token, unescaped) && trees.next().is_none()
        }
        _ => false,
    }
}

fn is_escaped_literal(token: &Token, unescaped: Symbol) -> bool {
    match match token {
        Token {
            kind: TokenKind::Literal(lit),
            span: _,
        } => Lit::from_lit_token(*lit, DUMMY_SP),
        Token {
            kind: TokenKind::Interpolated(nonterminal),
            span: _,
        } => match nonterminal.as_ref() {
            Nonterminal::NtExpr(expr) => match &expr.kind {
                ExprKind::Lit(lit) => Ok(lit.clone()),
                _ => return false,
            },
            _ => return false,
        },
        _ => return false,
    } {
        Ok(Lit {
            token:
                token::Lit {
                    kind: token::LitKind::Str,
                    symbol: _,
                    suffix: None,
                },
            kind: LitKind::Str(symbol, StrStyle::Cooked),
            span: _,
        }) => symbol.as_str().replace('\r', "") == unescaped.as_str().replace('\r', ""),
        _ => false,
    }
}

impl SpanlessEq for LazyTokenStream {
    fn eq(&self, other: &Self) -> bool {
        let this = self.create_token_stream();
        let other = other.create_token_stream();
        SpanlessEq::eq(&this, &other)
    }
}

impl SpanlessEq for AttrKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AttrKind::Normal(item, tokens), AttrKind::Normal(item2, tokens2)) => {
                SpanlessEq::eq(item, item2) && SpanlessEq::eq(tokens, tokens2)
            }
            (AttrKind::DocComment(kind, symbol), AttrKind::DocComment(kind2, symbol2)) => {
                SpanlessEq::eq(kind, kind2) && SpanlessEq::eq(symbol, symbol2)
            }
            (AttrKind::DocComment(kind, unescaped), AttrKind::Normal(item2, _tokens)) => {
                match kind {
                    CommentKind::Line | CommentKind::Block => {}
                }
                let path = Path::from_ident(Ident::with_dummy_span(sym::doc));
                SpanlessEq::eq(&path, &item2.path)
                    && match &item2.args {
                        MacArgs::Empty | MacArgs::Delimited(..) => false,
                        MacArgs::Eq(_span, token) => is_escaped_literal(token, *unescaped),
                    }
            }
            (AttrKind::Normal(..), AttrKind::DocComment(..)) => SpanlessEq::eq(other, self),
        }
    }
}
