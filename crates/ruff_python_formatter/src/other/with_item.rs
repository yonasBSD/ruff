use ruff_formatter::{FormatRuleWithOptions, write};
use ruff_python_ast::WithItem;

use crate::expression::maybe_parenthesize_expression;
use crate::expression::parentheses::{
    Parentheses, Parenthesize, is_expression_parenthesized, parenthesized,
};
use crate::prelude::*;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum WithItemLayout {
    /// A with item that is the `with`s only context manager and its context expression is parenthesized.
    ///
    /// ```python
    /// with (
    ///     a + b
    /// ) as b:
    ///     ...
    /// ```
    ///
    /// This layout is used independent of the target version.
    SingleParenthesizedContextManager,

    /// A with item that is the `with`s only context manager and it has no `target`.
    ///
    /// ```python
    /// with a + b:
    ///     ...
    /// ```
    ///
    /// In this case, use [`maybe_parenthesize_expression`] to get the same formatting as when
    /// formatting any other statement with a clause header.
    ///
    /// This layout is only used for Python 3.9+.
    ///
    /// Be careful that [`Self::SingleParenthesizedContextManager`] and this layout are compatible because
    /// removing optional parentheses or adding parentheses will make the formatter pick the opposite layout
    /// the second time the file gets formatted.
    SingleWithoutTarget,

    /// This layout is used when the target python version doesn't support parenthesized context managers and
    /// it's either a single, unparenthesized with item or multiple items.
    ///
    /// ```python
    /// with a + b:
    ///     ...
    ///
    /// with a, b:
    ///     ...
    /// ```
    Python38OrOlder { single: bool },

    /// A with item where the `with` formatting adds parentheses around all context managers if necessary.
    ///
    /// ```python
    /// with (
    ///     a,
    ///     b,
    /// ): pass
    /// ```
    ///
    /// This layout is generally used when the target version is Python 3.9 or newer, but it is used
    /// for Python 3.8 if the with item has a leading or trailing comment.
    ///
    /// ```python
    /// with (
    ///     # leading
    ///     a
    // ): ...
    /// ```
    ParenthesizedContextManagers { single: bool },
}

#[derive(Default)]
pub struct FormatWithItem {
    layout: WithItemLayout,
}

impl Default for WithItemLayout {
    fn default() -> Self {
        WithItemLayout::ParenthesizedContextManagers { single: false }
    }
}

impl FormatRuleWithOptions<WithItem, PyFormatContext<'_>> for FormatWithItem {
    type Options = WithItemLayout;

    fn with_options(self, options: Self::Options) -> Self {
        Self { layout: options }
    }
}

impl FormatNodeRule<WithItem> for FormatWithItem {
    fn fmt_fields(&self, item: &WithItem, f: &mut PyFormatter) -> FormatResult<()> {
        let WithItem {
            range: _,
            node_index: _,
            context_expr,
            optional_vars,
        } = item;

        let comments = f.context().comments().clone();
        let trailing_as_comments = comments.dangling(item);

        // WARNING: The `is_parenthesized` returns false-positives
        // if the `with` has a single item without a target.
        // E.g., it returns `true` for `with (a)` even though the parentheses
        // belong to the with statement and not the expression but it can't determine that.
        let is_parenthesized = is_expression_parenthesized(
            context_expr.into(),
            f.context().comments().ranges(),
            f.context().source(),
        );

        match self.layout {
            // Remove the parentheses of the `with_items` if the with statement adds parentheses
            WithItemLayout::ParenthesizedContextManagers { single } => {
                // ...except if the with item is parenthesized and it's not the only with item or it has a target.
                // Then use the context expression as a preferred breaking point.
                let prefer_breaking_context_expression =
                    (optional_vars.is_some() || !single) && is_parenthesized;

                if prefer_breaking_context_expression {
                    maybe_parenthesize_expression(
                        context_expr,
                        item,
                        Parenthesize::IfBreaksParenthesizedNested,
                    )
                    .fmt(f)?;
                } else {
                    context_expr
                        .format()
                        .with_options(Parentheses::Never)
                        .fmt(f)?;
                }
            }

            WithItemLayout::SingleParenthesizedContextManager
            | WithItemLayout::SingleWithoutTarget => {
                write!(
                    f,
                    [maybe_parenthesize_expression(
                        context_expr,
                        item,
                        Parenthesize::IfBreaks
                    )]
                )?;
            }

            WithItemLayout::Python38OrOlder { single } => {
                let parenthesize = if single || is_parenthesized {
                    Parenthesize::IfBreaks
                } else {
                    Parenthesize::IfRequired
                };
                write!(
                    f,
                    [maybe_parenthesize_expression(
                        context_expr,
                        item,
                        parenthesize
                    )]
                )?;
            }
        }

        if let Some(optional_vars) = optional_vars {
            write!(f, [space(), token("as"), space()])?;

            if trailing_as_comments.is_empty() {
                write!(f, [optional_vars.format()])?;
            } else {
                write!(
                    f,
                    [parenthesized(
                        "(",
                        &optional_vars.format().with_options(Parentheses::Never),
                        ")",
                    )
                    .with_dangling_comments(trailing_as_comments)]
                )?;
            }
        }

        Ok(())
    }
}
