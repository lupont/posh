use std::ops::RangeInclusive;
use std::os::fd::IntoRawFd;
use std::os::fd::RawFd;

#[cfg(feature = "serde")]
use serde::Serialize;

use crate::engine::builtin;
use crate::engine::expand::remove_quotes;
use crate::engine::expand::Expand;
use crate::Engine;
use crate::Error;

/// ```[no_run]
/// program : linebreak complete_commands linebreak
///         | linebreak
///         ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyntaxTree {
    pub leading: Linebreak,
    pub commands: Option<(CompleteCommands, Linebreak)>,
    pub unparsed: String,
}

impl SyntaxTree {
    pub fn is_ok(&self) -> bool {
        self.unparsed.chars().all(char::is_whitespace)
    }

    #[cfg(feature = "serde")]
    pub fn as_json(&self) -> crate::Result<String> {
        let json = serde_json::to_string(&self)?;
        Ok(json)
    }
}

/// ```[no_run]
/// complete_commands : complete_commands newline_list complete_command
///                   |                                complete_command
///                   ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteCommands {
    pub head: CompleteCommand,
    pub tail: Vec<(NewlineList, CompleteCommand)>,
}

impl CompleteCommands {
    pub fn full(self) -> Vec<CompleteCommand> {
        let mut v = vec![self.head];
        for (_, cmd) in self.tail {
            v.push(cmd);
        }
        v
    }
}

/// ```[no_run]
/// complete_command : list separator_op
///                  | list
///                  ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompleteCommand {
    List {
        list: List,
        separator_op: Option<SeparatorOp>,
        comment: Option<Comment>,
    },

    Comment {
        comment: Comment,
    },
}

impl CompleteCommand {
    pub fn list_with_separator(self) -> Vec<(AndOrList, SeparatorOp)> {
        let mut items = Vec::new();

        let (list, separator_op) = match self {
            Self::List {
                list, separator_op, ..
            } => (list, separator_op),
            Self::Comment { .. } => return items,
        };

        let final_separator = match separator_op {
            Some(separator) => separator,
            None => Default::default(),
        };

        if list.tail.is_empty() {
            items.push((list.head, final_separator));
        } else {
            let mut prev_list = list.head;

            for (sep, and_or_list) in list.tail {
                items.push((prev_list, sep));
                prev_list = and_or_list;
            }

            items.push((prev_list, final_separator));
        }

        items
    }
}

/// ```[no_run]
/// list : list separator_op and_or
///      |                   and_or
///      ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct List {
    pub head: AndOrList,
    pub tail: Vec<(SeparatorOp, AndOrList)>,
}

/// ```[no_run]
/// and_or :                         pipeline
///        | and_or AND_IF linebreak pipeline
///        | and_or OR_IF  linebreak pipeline
///        ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AndOrList {
    pub head: Pipeline,

    // As noted semantically by having it be the first part of
    // the tuple, each `LogicalOp` here operates on the previous
    // `Pipeline` and it's tuple partner.
    pub tail: Vec<(LogicalOp, Linebreak, Pipeline)>,
}

/// ```[no_run]
/// pipeline :      pipe_sequence
///          | Bang pipe_sequence
///          ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pipeline {
    pub bang: Option<Bang>,
    pub sequence: PipeSequence,
}

impl Pipeline {
    /// Always at least one in length, since this joins self.first and self.rest.
    pub fn full(self) -> Vec<Command> {
        let mut v = vec![*self.sequence.head];
        for (_, _, cmd) in self.sequence.tail {
            v.push(cmd);
        }
        v
    }

    pub fn has_bang(&self) -> bool {
        self.bang.is_some()
    }
}

/// ```[no_run]
/// pipe_sequence :                             command
///               | pipe_sequence '|' linebreak command
///               ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PipeSequence {
    pub head: Box<Command>,
    pub tail: Vec<(Pipe, Linebreak, Command)>,
}

/// ```[no_run]
/// command : simple_command
///         | compound_command
///         | compound_command redirect_list
///         | function_definition
///         ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Simple(SimpleCommand),
    Compound(CompoundCommand, Vec<Redirection>),
    FunctionDefinition(FunctionDefinition),
}

impl Default for Command {
    fn default() -> Self {
        Self::Simple(SimpleCommand::default())
    }
}

/// ```[no_run]
/// compound_command : brace_group
///                  | subshell
///                  | for_clause
///                  | case_clause
///                  | if_clause
///                  | while_clause
///                  | until_clause
///                  ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompoundCommand {
    Brace(BraceGroup),
    Subshell(Subshell),
    For(ForClause),
    Case(CaseClause),
    If(IfClause),
    While(WhileClause),
    Until(UntilClause),
}

impl Default for CompoundCommand {
    fn default() -> Self {
        Self::Brace(BraceGroup::default())
    }
}

/// ```[no_run]
/// subshell : '(' compound_list ')'
///          ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Subshell {
    pub lparen_ws: LeadingWhitespace,
    pub body: CompoundList,
    pub rparen_ws: LeadingWhitespace,
}

/// ```[no_run]
/// compound_list : linebreak term
///               | linebreak term separator
///               ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct CompoundList {
    pub linebreak: Linebreak,
    pub term: Term,
    pub separator: Option<Separator>,
}

/// ```[no_run]
/// term : term separator and_or
///      |                and_or
///      ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Term {
    pub head: AndOrList,
    pub tail: Vec<(Separator, AndOrList)>,
}

/// ```[no_run]
/// for_clause : For name                                      do_group
///            | For name                       sequential_sep do_group
///            | For name linebreak in          sequential_sep do_group
///            | For name linebreak in wordlist sequential_sep do_group
///            ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum ForClause {
    Simple(Name, DoGroup),
    Padded(Name, SequentialSeparator, DoGroup),
    Full(Name, Linebreak, Vec<Word>, SequentialSeparator, DoGroup),
}

/// ```[no_run]
/// name : NAME /* Apply rule 5 */
///      ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Name {
    #[cfg_attr(feature = "serde", serde(rename = "leading_whitespace"))]
    pub whitespace: LeadingWhitespace,
    pub name: String,
}

/// ```[no_run]
/// case_clause : Case WORD linebreak in linebreak case_list    Esac
///             | Case WORD linebreak in linebreak case_list_ns Esac
///             | Case WORD linebreak in linebreak              Esac
///             ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum CaseClause {
    Normal(Word, Linebreak, Linebreak, CaseList),
    NoSeparator(Word, Linebreak, Linebreak, CaseListNs),
    Empty(Word, Linebreak, Linebreak),
}

/// ```[no_run]
/// case_list_ns : case_list case_item_ns
///              |           case_item_ns
///              ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct CaseListNs {
    pub case_list: Option<CaseList>,
    pub last: CaseItemNs,
}

/// ```[no_run]
/// case_list : case_list case_item
///           |           case_item
///           ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct CaseList {
    pub head: CaseItem,
    pub tail: Vec<CaseItem>,
}

/// ```[no_run]
/// case_item_ns :     pattern ')' linebreak
///              |     pattern ')' compound_list
///              | '(' pattern ')' linebreak
///              | '(' pattern ')' compound_list
///              ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum CaseItemNs {
    Empty(bool, Pattern, Linebreak),
    List(bool, Pattern, CompoundList),
}

/// ```[no_run]
/// case_item :     pattern ')' linebreak     DSEMI linebreak
///           |     pattern ')' compound_list DSEMI linebreak
///           | '(' pattern ')' linebreak     DSEMI linebreak
///           | '(' pattern ')' compound_list DSEMI linebreak
///           ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum CaseItem {
    Empty(bool, Pattern, Linebreak, Linebreak),
    List(bool, Pattern, CompoundList, Linebreak),
}

/// ```[no_run]
/// pattern :             WORD /* Apply rule 4 */
///         | pattern '|' WORD /* Do not apply rule 4 */
///         ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Pattern {
    pub head: Word,
    pub tail: Vec<Word>,
}

/// ```[no_run]
/// if_clause : If compound_list Then compound_list else_part Fi
///           | If compound_list Then compound_list           Fi
///           ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct IfClause {
    pub predicate: CompoundList,
    pub body: CompoundList,
    pub else_part: Option<ElsePart>,
}

/// ```[no_run]
/// else_part : Elif compound_list Then compound_list
///           | Elif compound_list Then compound_list else_part
///           | Else compound_list
///           ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct ElsePart {
    pub elseifs: Vec<(CompoundList, CompoundList)>,
    pub else_part: Option<CompoundList>,
}

/// ```[no_run]
/// while_clause : While compound_list do_group
///              ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct WhileClause {
    pub predicate: CompoundList,
    pub body: DoGroup,
}

/// ```[no_run]
/// until_clause : Until compound_list do_group
///              ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct UntilClause {
    pub predicate: CompoundList,
    pub body: DoGroup,
}

/// ```[no_run]
/// function_definition : fname '(' ')' linebreak function_body
///                     ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct FunctionDefinition {
    pub name: Name,
    pub parens: String,
    pub linebreak: Linebreak,
    pub body: FunctionBody,
}

/// ```[no_run]
/// function_body : compound_command               /* Apply rule 9 */
///               | compound_command redirect_list /* Apply rule 9 */
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct FunctionBody {
    pub command: CompoundCommand,
    pub redirections: Vec<Redirection>,
}

/// ```[no_run]
/// brace_group : Lbrace compound_list Rbrace
///             ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct BraceGroup {
    pub lbrace_ws: LeadingWhitespace,
    pub body: CompoundList,
    pub rbrace_ws: LeadingWhitespace,
}

/// ```[no_run]
/// do_group : Do compound_list Done /* Apply rule 6 */
///          ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct DoGroup {
    pub body: CompoundList,
}

/// ```[no_run]
/// simple_command : cmd_prefix cmd_word cmd_suffix
///                | cmd_prefix cmd_word
///                | cmd_prefix
///                | cmd_name cmd_suffix
///                | cmd_name
///                ;
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct SimpleCommand {
    pub name: Option<Word>,
    pub prefixes: Vec<CmdPrefix>,
    pub suffixes: Vec<CmdSuffix>,
}

impl SimpleCommand {
    pub fn name(&self) -> Option<&String> {
        if let Some(word) = &self.name {
            Some(&word.name)
        } else {
            None
        }
    }

    pub fn expand_into_args(&self, engine: &mut Engine) -> Vec<String> {
        let mut args = Vec::new();

        if let Some(name) = self.name.clone() {
            let mut expanded = name.expand(engine);
            args.append(&mut expanded);
        }

        for suffix in &self.suffixes {
            if let CmdSuffix::Word(word) = suffix.clone() {
                let mut expanded = word.expand(engine);
                args.append(&mut expanded);
            }
        }

        args
    }

    pub fn assignments(&self) -> impl Iterator<Item = &VariableAssignment> {
        self.prefixes.iter().filter_map(|m| match m {
            CmdPrefix::Assignment(a) => Some(a),
            _ => None,
        })
    }

    pub fn redirections(&self) -> impl Iterator<Item = &Redirection> {
        self.prefixes
            .iter()
            .filter_map(|m| match m {
                CmdPrefix::Redirection(r) => Some(r),
                _ => None,
            })
            .chain(self.suffixes.iter().filter_map(|m| match m {
                CmdSuffix::Redirection(r) => Some(r),
                _ => None,
            }))
    }

    pub fn is_builtin(&self) -> bool {
        matches!(&self.name, Some(Word { name, .. }) if builtin::has(&remove_quotes(name, false).unwrap()))
    }
}

/// ```[no_run]
/// cmd_prefix :            io_redirect
///            | cmd_prefix io_redirect
///            |            ASSIGNMENT_WORD
///            | cmd_prefix ASSIGNMENT_WORD
///            ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CmdPrefix {
    Redirection(Redirection),
    Assignment(VariableAssignment),
}

/// ```[no_run]
/// cmd_suffix :            io_redirect
///            | cmd_suffix io_redirect
///            |            WORD
///            | cmd_suffix WORD
///            ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CmdSuffix {
    Redirection(Redirection),
    Word(Word),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileDescriptor {
    Stdin,
    Stdout,
    Stderr,
    Other(i32),
}

impl FileDescriptor {
    pub fn try_from(input: &str) -> Option<Self> {
        if input.chars().all(|c| c.is_ascii_digit()) {
            input.parse::<i32>().ok().map(Into::into)
        } else {
            None
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        match self {
            FileDescriptor::Stdin => 0,
            FileDescriptor::Stdout => 1,
            FileDescriptor::Stderr => 2,
            FileDescriptor::Other(n) => *n,
        }
    }

    pub fn is_stdin(&self) -> bool {
        matches!(self, Self::Stdin)
    }

    pub fn is_stdout(&self) -> bool {
        matches!(self, Self::Stdout)
    }

    pub fn is_stderr(&self) -> bool {
        matches!(self, Self::Stderr)
    }
}

impl From<RawFd> for FileDescriptor {
    fn from(value: RawFd) -> Self {
        match value {
            0 => Self::Stdin,
            1 => Self::Stdout,
            2 => Self::Stderr,
            n => Self::Other(n),
        }
    }
}

/// `Input`:         `<`
/// `InputFd`:       `<&`
/// `ReadWrite`:     `<>`
/// `Output`:        `>`
/// `OutputFd`:      `>&`
/// `OutputAppend`:  `>>`
/// `OutputClobber`: `>|`
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum RedirectionType {
    #[cfg_attr(feature = "serde", serde(rename = "input"))]
    /// `<`
    Input,

    #[cfg_attr(feature = "serde", serde(rename = "input_from_fd"))]
    /// `<&`
    InputFd,

    #[cfg_attr(feature = "serde", serde(rename = "input_output"))]
    /// `<>`
    ReadWrite,

    #[cfg_attr(feature = "serde", serde(rename = "output"))]
    /// `>`
    Output,

    #[cfg_attr(feature = "serde", serde(rename = "output_to_fd"))]
    /// `>&`
    OutputFd,

    #[cfg_attr(feature = "serde", serde(rename = "output_append"))]
    /// `>>`
    OutputAppend,

    #[cfg_attr(feature = "serde", serde(rename = "output_clobber"))]
    /// `>|`
    OutputClobber,
}

impl RedirectionType {
    pub fn default_dst_fd(&self) -> FileDescriptor {
        match self {
            Self::Input => FileDescriptor::Stdin,
            Self::InputFd => FileDescriptor::Stdin,
            Self::ReadWrite => FileDescriptor::Stdin,
            Self::Output => FileDescriptor::Stdout,
            Self::OutputFd => FileDescriptor::Stdout,
            Self::OutputAppend => FileDescriptor::Stdout,
            Self::OutputClobber => FileDescriptor::Stdout,
        }
    }

    pub fn default_src_fd(&self, path: &str) -> crate::Result<FileDescriptor> {
        let mut options = std::fs::OpenOptions::new();
        match self {
            Self::InputFd => {
                if let Some(fd) = FileDescriptor::try_from(path) {
                    return Ok(fd);
                } else {
                    options.read(true);
                }
            }
            Self::OutputFd => {
                if let Some(fd) = FileDescriptor::try_from(path) {
                    return Ok(fd);
                } else {
                    options.write(true).truncate(true).create(true);
                }
            }
            Self::Input => {
                options.read(true);
            }
            Self::ReadWrite => {
                options.read(true).write(true).create(true);
            }
            Self::Output => {
                options.write(true).truncate(true).create(true);
            }
            Self::OutputClobber => {
                options.write(true).truncate(true).create(true);
            }
            Self::OutputAppend => {
                options.write(true).append(true).create(true);
            }
        }
        Ok(options
            .open(path)
            .map_err(|_| Error::NonExistentFile(path.to_string()))?
            .into_raw_fd()
            .into())
    }
}

/// `Normal`:    `<<`
/// `StripTabs`: `<<-`
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum HereDocType {
    /// `<<`
    Normal,

    /// `<<-`
    StripTabs,
}

/// ```[no_run]
/// io_redirect :           io_file
///             | IO_NUMBER io_file
///             |           io_here
///             | IO_NUMBER io_here
///             ;
///
/// io_file : '<'       filename
///         | LESSAND   filename
///         | '>'       filename
///         | GREATAND  filename
///         | DGREAT    filename
///         | LESSGREAT filename
///         | CLOBBER   filename
///         ;
///
/// io_here : DLESS     here_end
///         | DLESSDASH here_end
///         ;
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum Redirection {
    #[cfg_attr(feature = "serde", serde(rename = "fd_redirection"))]
    File {
        whitespace: LeadingWhitespace,
        input_fd: Option<FileDescriptor>,
        #[cfg_attr(feature = "serde", serde(rename = "type"))]
        ty: RedirectionType,
        target: Word,
    },

    #[cfg_attr(feature = "serde", serde(rename = "here_doc"))]
    Here {
        whitespace: LeadingWhitespace,
        input_fd: Option<FileDescriptor>,

        #[cfg_attr(feature = "serde", serde(rename = "type"))]
        ty: HereDocType,

        /// The delimiter
        end: Word,

        /// The entire content of the here document
        content: Word,
    },
}

impl Redirection {
    pub fn new_file(
        whitespace: impl Into<LeadingWhitespace>,
        input_fd: Option<FileDescriptor>,
        ty: RedirectionType,
        target: Word,
    ) -> Self {
        Self::File {
            whitespace: whitespace.into(),
            input_fd,
            ty,
            target,
        }
    }

    pub fn new_input(
        whitespace: impl Into<LeadingWhitespace>,
        fd: Option<FileDescriptor>,
        target: Word,
    ) -> Self {
        Self::new_file(whitespace, fd, RedirectionType::Input, target)
    }

    pub fn new_output(
        whitespace: impl Into<LeadingWhitespace>,
        fd: Option<FileDescriptor>,
        target: Word,
    ) -> Self {
        Self::new_file(whitespace, fd, RedirectionType::Output, target)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct VariableAssignment {
    pub whitespace: LeadingWhitespace,
    pub lhs: Name,
    pub rhs: Option<Word>,
}

impl VariableAssignment {
    pub fn new(lhs: Name, rhs: Option<Word>, whitespace: impl Into<LeadingWhitespace>) -> Self {
        Self {
            whitespace: whitespace.into(),
            lhs,
            rhs,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Word {
    pub whitespace: LeadingWhitespace,
    pub name: String,
    pub expansions: Vec<Expansion>,
}

impl Word {
    pub fn new(input: &str, whitespace: impl Into<LeadingWhitespace>) -> Self {
        Self {
            whitespace: whitespace.into(),
            name: input.to_string(),
            expansions: Default::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum Expansion {
    Tilde {
        range: RangeInclusive<usize>,
        name: String,
    },

    Glob {
        range: RangeInclusive<usize>,
        recursive: bool,
        pattern: String,
    },

    Brace {
        range: RangeInclusive<usize>,
        pattern: String,
    },

    Parameter {
        range: RangeInclusive<usize>,
        name: String,
        finished: bool,
        quoted: bool,
    },

    Command {
        range: RangeInclusive<usize>,
        part: String,
        tree: SyntaxTree,
        finished: bool,
        quoted: bool,
    },

    Arithmetic {
        range: RangeInclusive<usize>,
        expression: Word,
        finished: bool,
        quoted: bool,
    },
}

impl Expansion {
    pub fn is_finished(&self) -> bool {
        match self {
            Self::Tilde { .. } => true,
            Self::Glob { .. } => true,
            Self::Brace { .. } => true,
            Self::Parameter { finished, .. } => *finished,
            Self::Command { finished, .. } => *finished,
            Self::Arithmetic { finished, .. } => *finished,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogicalOp {
    And(LeadingWhitespace),
    Or(LeadingWhitespace),
}

/// newline_list :              NEWLINE
///              | newline_list NEWLINE
///              ;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewlineList {
    /// This String may contain a mix of ' ', \t, and \n
    pub whitespace: String,
}

/// linebreak : newline_list
///           | /* empty */
///           ;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Linebreak {
    pub newlines: Option<NewlineList>,
}

/// separator_op : '&'
///              | ';'
///              ;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeparatorOp {
    Sync(LeadingWhitespace),
    Async(LeadingWhitespace),
}

impl SeparatorOp {
    pub fn is_sync(&self) -> bool {
        matches!(self, Self::Sync(_))
    }

    pub fn is_async(&self) -> bool {
        !self.is_sync()
    }
}

impl Default for SeparatorOp {
    fn default() -> Self {
        Self::Sync(Default::default())
    }
}

/// separator : separator_op linebreak
///           | newline_list
///           ;
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Separator {
    Explicit(SeparatorOp, Linebreak),
    Implicit(NewlineList),
}

/// sequential_sep : ';' linebreak
///                | newline_list
///                ;
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub enum SequentialSeparator {
    Semi(Linebreak),
    Implicit(NewlineList),
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Bang {
    #[cfg_attr(feature = "serde", serde(rename = "leading_whitespace"))]
    pub whitespace: LeadingWhitespace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Comment {
    #[cfg_attr(feature = "serde", serde(rename = "leading_whitespace"))]
    pub whitespace: LeadingWhitespace,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
pub struct Pipe {
    #[cfg_attr(feature = "serde", serde(rename = "leading_whitespace"))]
    pub whitespace: LeadingWhitespace,
}

/// Wrapper type for String, used by data structures
/// that keep track of leading whitespace.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LeadingWhitespace(pub String);

impl std::fmt::Display for LeadingWhitespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for LeadingWhitespace {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<&str> for LeadingWhitespace {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
