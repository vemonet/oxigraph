//! A [N3](https://w3c.github.io/N3/spec/) streaming parser implemented by [`N3Parser`].

use crate::lexer::{resolve_local_name, N3Lexer, N3LexerMode, N3LexerOptions, N3Token};
#[cfg(feature = "async-tokio")]
use crate::toolkit::FromTokioAsyncReadIterator;
use crate::toolkit::{
    FromReadIterator, Lexer, Parser, RuleRecognizer, RuleRecognizerError, SyntaxError,
};
use crate::{ParseError, MAX_BUFFER_SIZE, MIN_BUFFER_SIZE};
use oxiri::{Iri, IriParseError};
use oxrdf::vocab::{rdf, xsd};
#[cfg(feature = "rdf-star")]
use oxrdf::Triple;
use oxrdf::{
    BlankNode, GraphName, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, Subject, Term,
    Variable,
};
use std::collections::HashMap;
use std::fmt;
use std::io::Read;
#[cfg(feature = "async-tokio")]
use tokio::io::AsyncRead;

/// A N3 term i.e. a RDF `Term` or a `Variable`.
#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub enum N3Term {
    NamedNode(NamedNode),
    BlankNode(BlankNode),
    Literal(Literal),
    #[cfg(feature = "rdf-star")]
    Triple(Box<Triple>),
    Variable(Variable),
}

impl fmt::Display for N3Term {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NamedNode(term) => term.fmt(f),
            Self::BlankNode(term) => term.fmt(f),
            Self::Literal(term) => term.fmt(f),
            #[cfg(feature = "rdf-star")]
            Self::Triple(term) => term.fmt(f),
            Self::Variable(term) => term.fmt(f),
        }
    }
}

impl From<NamedNode> for N3Term {
    #[inline]
    fn from(node: NamedNode) -> Self {
        Self::NamedNode(node)
    }
}

impl From<NamedNodeRef<'_>> for N3Term {
    #[inline]
    fn from(node: NamedNodeRef<'_>) -> Self {
        Self::NamedNode(node.into_owned())
    }
}

impl From<BlankNode> for N3Term {
    #[inline]
    fn from(node: BlankNode) -> Self {
        Self::BlankNode(node)
    }
}

impl From<Literal> for N3Term {
    #[inline]
    fn from(literal: Literal) -> Self {
        Self::Literal(literal)
    }
}

#[cfg(feature = "rdf-star")]
impl From<Triple> for N3Term {
    #[inline]
    fn from(triple: Triple) -> Self {
        Self::Triple(Box::new(triple))
    }
}

#[cfg(feature = "rdf-star")]
impl From<Box<Triple>> for N3Term {
    #[inline]
    fn from(node: Box<Triple>) -> Self {
        Self::Triple(node)
    }
}

impl From<NamedOrBlankNode> for N3Term {
    #[inline]
    fn from(node: NamedOrBlankNode) -> Self {
        match node {
            NamedOrBlankNode::NamedNode(node) => node.into(),
            NamedOrBlankNode::BlankNode(node) => node.into(),
        }
    }
}

impl From<Subject> for N3Term {
    #[inline]
    fn from(node: Subject) -> Self {
        match node {
            Subject::NamedNode(node) => node.into(),
            Subject::BlankNode(node) => node.into(),
            #[cfg(feature = "rdf-star")]
            Subject::Triple(triple) => Self::Triple(triple),
        }
    }
}

impl From<Term> for N3Term {
    #[inline]
    fn from(node: Term) -> Self {
        match node {
            Term::NamedNode(node) => node.into(),
            Term::BlankNode(node) => node.into(),
            Term::Literal(node) => node.into(),
            #[cfg(feature = "rdf-star")]
            Term::Triple(triple) => Self::Triple(triple),
        }
    }
}

impl From<Variable> for N3Term {
    #[inline]
    fn from(variable: Variable) -> Self {
        Self::Variable(variable)
    }
}

/// A N3 quad i.e. a quad composed of [`N3Term`].
///
/// The `graph_name` is used to encode the formula where the triple is in.
/// In this case the formula is encoded by a blank node.
#[derive(Eq, PartialEq, Debug, Clone, Hash)]
pub struct N3Quad {
    /// The [subject](https://www.w3.org/TR/rdf11-concepts/#dfn-subject) of this triple.
    pub subject: N3Term,

    /// The [predicate](https://www.w3.org/TR/rdf11-concepts/#dfn-predicate) of this triple.
    pub predicate: N3Term,

    /// The [object](https://www.w3.org/TR/rdf11-concepts/#dfn-object) of this triple.
    pub object: N3Term,

    /// The name of the RDF [graph](https://www.w3.org/TR/rdf11-concepts/#dfn-rdf-graph) in which the triple is.
    pub graph_name: GraphName,
}

impl fmt::Display for N3Quad {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.graph_name == GraphName::DefaultGraph {
            write!(f, "{} {} {}", self.subject, self.predicate, self.object)
        } else {
            write!(
                f,
                "{} {} {} {}",
                self.subject, self.predicate, self.object, self.graph_name
            )
        }
    }
}

impl From<Quad> for N3Quad {
    fn from(quad: Quad) -> Self {
        Self {
            subject: quad.subject.into(),
            predicate: quad.predicate.into(),
            object: quad.object.into(),
            graph_name: quad.graph_name,
        }
    }
}

/// A [N3](https://w3c.github.io/N3/spec/) streaming parser.
///
/// Count the number of people:
/// ```
/// use oxrdf::{NamedNode, vocab::rdf};
/// use oxttl::n3::{N3Parser, N3Term};
///
/// let file = b"@base <http://example.com/> .
/// @prefix schema: <http://schema.org/> .
/// <foo> a schema:Person ;
///     schema:name \"Foo\" .
/// <bar> a schema:Person ;
///     schema:name \"Bar\" .";
///
/// let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
/// let schema_person = N3Term::NamedNode(NamedNode::new("http://schema.org/Person")?);
/// let mut count = 0;
/// for triple in N3Parser::new().parse_read(file.as_ref()) {
///     let triple = triple?;
///     if triple.predicate == rdf_type && triple.object == schema_person {
///         count += 1;
///     }
/// }
/// assert_eq!(2, count);
/// # Result::<_,Box<dyn std::error::Error>>::Ok(())
/// ```
#[derive(Default)]
pub struct N3Parser {
    base: Option<Iri<String>>,
    prefixes: HashMap<String, Iri<String>>,
}

impl N3Parser {
    /// Builds a new [`N3Parser`].
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn with_base_iri(mut self, base_iri: impl Into<String>) -> Result<Self, IriParseError> {
        self.base = Some(Iri::parse(base_iri.into())?);
        Ok(self)
    }

    #[inline]
    pub fn with_prefix(
        mut self,
        prefix_name: impl Into<String>,
        prefix_iri: impl Into<String>,
    ) -> Result<Self, IriParseError> {
        self.prefixes
            .insert(prefix_name.into(), Iri::parse(prefix_iri.into())?);
        Ok(self)
    }

    /// Parses a N3 file from a [`Read`] implementation.
    ///
    /// Count the number of people:
    /// ```
    /// use oxrdf::NamedNode;
    /// use oxttl::n3::{N3Parser, N3Term};
    ///
    /// let file = b"@base <http://example.com/> .
    /// @prefix schema: <http://schema.org/> .
    /// <foo> a schema:Person ;
    ///     schema:name \"Foo\" .
    /// <bar> a schema:Person ;
    ///     schema:name \"Bar\" .";
    ///
    /// let rdf_type = N3Term::NamedNode(NamedNode::new("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")?);
    /// let schema_person = N3Term::NamedNode(NamedNode::new("http://schema.org/Person")?);
    /// let mut count = 0;
    /// for triple in N3Parser::new().parse_read(file.as_ref()) {
    ///     let triple = triple?;
    ///     if triple.predicate == rdf_type && triple.object == schema_person {
    ///         count += 1;
    ///     }
    /// }
    /// assert_eq!(2, count);
    /// # Result::<_,Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn parse_read<R: Read>(&self, read: R) -> FromReadN3Reader<R> {
        FromReadN3Reader {
            inner: self.parse().parser.parse_read(read),
        }
    }

    /// Parses a N3 file from a [`AsyncRead`] implementation.
    ///
    /// Count the number of people:
    /// ```
    /// use oxrdf::{NamedNode, vocab::rdf};
    /// use oxttl::n3::{N3Parser, N3Term};
    /// use oxttl::ParseError;
    ///
    /// #[tokio::main(flavor = "current_thread")]
    /// async fn main() -> Result<(), ParseError> {
    ///     let file = b"@base <http://example.com/> .
    ///     @prefix schema: <http://schema.org/> .
    ///     <foo> a schema:Person ;
    ///         schema:name \"Foo\" .
    ///     <bar> a schema:Person ;
    ///         schema:name \"Bar\" .";
    ///
    ///     let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
    ///     let schema_person = N3Term::NamedNode(NamedNode::new_unchecked("http://schema.org/Person"));
    ///     let mut count = 0;
    ///     let mut parser = N3Parser::new().parse_tokio_async_read(file.as_ref());
    ///     while let Some(triple) = parser.next().await {
    ///         let triple = triple?;
    ///         if triple.predicate == rdf_type && triple.object == schema_person {
    ///             count += 1;
    ///         }
    ///     }
    ///     assert_eq!(2, count);
    ///     Ok(())
    /// }
    /// ```
    #[cfg(feature = "async-tokio")]
    pub fn parse_tokio_async_read<R: AsyncRead + Unpin>(
        &self,
        read: R,
    ) -> FromTokioAsyncReadN3Reader<R> {
        FromTokioAsyncReadN3Reader {
            inner: self.parse().parser.parse_tokio_async_read(read),
        }
    }

    /// Allows to parse a N3 file by using a low-level API.
    ///
    /// Count the number of people:
    /// ```
    /// use oxrdf::{NamedNode, vocab::rdf};
    /// use oxttl::n3::{N3Parser, N3Term};
    ///
    /// let file: [&[u8]; 5] = [b"@base <http://example.com/>",
    ///     b". @prefix schema: <http://schema.org/> .",
    ///     b"<foo> a schema:Person",
    ///     b" ; schema:name \"Foo\" . <bar>",
    ///     b" a schema:Person ; schema:name \"Bar\" ."
    /// ];
    ///
    /// let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
    /// let schema_person = N3Term::NamedNode(NamedNode::new("http://schema.org/Person")?);
    /// let mut count = 0;
    /// let mut parser = N3Parser::new().parse();
    /// let mut file_chunks = file.iter();
    /// while !parser.is_end() {
    ///     // We feed more data to the parser
    ///     if let Some(chunk) = file_chunks.next() {
    ///         parser.extend_from_slice(chunk);    
    ///     } else {
    ///         parser.end(); // It's finished
    ///     }
    ///     // We read as many triples from the parser as possible
    ///     while let Some(triple) = parser.read_next() {
    ///         let triple = triple?;
    ///         if triple.predicate == rdf_type && triple.object == schema_person {
    ///             count += 1;
    ///         }
    ///     }
    /// }
    /// assert_eq!(2, count);
    /// # Result::<_,Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn parse(&self) -> LowLevelN3Reader {
        LowLevelN3Reader {
            parser: N3Recognizer::new_parser(self.base.clone(), self.prefixes.clone()),
        }
    }
}

/// Parses a N3 file from a [`Read`] implementation. Can be built using [`N3Parser::parse_read`].
///
/// Count the number of people:
/// ```
/// use oxrdf::{NamedNode, vocab::rdf};
/// use oxttl::n3::{N3Parser, N3Term};
///
/// let file = b"@base <http://example.com/> .
/// @prefix schema: <http://schema.org/> .
/// <foo> a schema:Person ;
///     schema:name \"Foo\" .
/// <bar> a schema:Person ;
///     schema:name \"Bar\" .";
///
/// let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
/// let schema_person = N3Term::NamedNode(NamedNode::new("http://schema.org/Person")?);
/// let mut count = 0;
/// for triple in N3Parser::new().parse_read(file.as_ref()) {
///     let triple = triple?;
///     if triple.predicate == rdf_type && triple.object == schema_person {
///         count += 1;
///     }
/// }
/// assert_eq!(2, count);
/// # Result::<_,Box<dyn std::error::Error>>::Ok(())
/// ```
pub struct FromReadN3Reader<R: Read> {
    inner: FromReadIterator<R, N3Recognizer>,
}

impl<R: Read> Iterator for FromReadN3Reader<R> {
    type Item = Result<N3Quad, ParseError>;

    fn next(&mut self) -> Option<Result<N3Quad, ParseError>> {
        self.inner.next()
    }
}

/// Parses a N3 file from a [`AsyncRead`] implementation. Can be built using [`N3Parser::parse_tokio_async_read`].
///
/// Count the number of people:
/// ```
/// use oxrdf::{NamedNode, vocab::rdf};
/// use oxttl::n3::{N3Parser, N3Term};
/// use oxttl::ParseError;
///
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() -> Result<(), ParseError> {
///     let file = b"@base <http://example.com/> .
///     @prefix schema: <http://schema.org/> .
///     <foo> a schema:Person ;
///         schema:name \"Foo\" .
///     <bar> a schema:Person ;
///         schema:name \"Bar\" .";
///
///     let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
///     let schema_person = N3Term::NamedNode(NamedNode::new_unchecked("http://schema.org/Person"));
///     let mut count = 0;
///     let mut parser = N3Parser::new().parse_tokio_async_read(file.as_ref());
///     while let Some(triple) = parser.next().await {
///         let triple = triple?;
///         if triple.predicate == rdf_type && triple.object == schema_person {
///             count += 1;
///         }
///     }
///     assert_eq!(2, count);
///     Ok(())
/// }
/// ```
#[cfg(feature = "async-tokio")]
pub struct FromTokioAsyncReadN3Reader<R: AsyncRead + Unpin> {
    inner: FromTokioAsyncReadIterator<R, N3Recognizer>,
}

#[cfg(feature = "async-tokio")]
impl<R: AsyncRead + Unpin> FromTokioAsyncReadN3Reader<R> {
    /// Reads the next triple or returns `None` if the file is finished.
    pub async fn next(&mut self) -> Option<Result<N3Quad, ParseError>> {
        Some(self.inner.next().await?.map(Into::into))
    }
}

/// Parses a N3 file by using a low-level API. Can be built using [`N3Parser::parse`].
///
/// Count the number of people:
/// ```
/// use oxrdf::{NamedNode, vocab::rdf};
/// use oxttl::n3::{N3Parser, N3Term};
///
/// let file: [&[u8]; 5] = [b"@base <http://example.com/>",
///     b". @prefix schema: <http://schema.org/> .",
///     b"<foo> a schema:Person",
///     b" ; schema:name \"Foo\" . <bar>",
///     b" a schema:Person ; schema:name \"Bar\" ."
/// ];
///
/// let rdf_type = N3Term::NamedNode(rdf::TYPE.into_owned());
/// let schema_person = N3Term::NamedNode(NamedNode::new("http://schema.org/Person")?);
/// let mut count = 0;
/// let mut parser = N3Parser::new().parse();
/// let mut file_chunks = file.iter();
/// while !parser.is_end() {
///     // We feed more data to the parser
///     if let Some(chunk) = file_chunks.next() {
///         parser.extend_from_slice(chunk);    
///     } else {
///         parser.end(); // It's finished
///     }
///     // We read as many triples from the parser as possible
///     while let Some(triple) = parser.read_next() {
///         let triple = triple?;
///         if triple.predicate == rdf_type && triple.object == schema_person {
///             count += 1;
///         }
///     }
/// }
/// assert_eq!(2, count);
/// # Result::<_,Box<dyn std::error::Error>>::Ok(())
/// ```
pub struct LowLevelN3Reader {
    parser: Parser<N3Recognizer>,
}

impl LowLevelN3Reader {
    /// Adds some extra bytes to the parser. Should be called when [`read_next`](Self::read_next) returns [`None`] and there is still unread data.
    pub fn extend_from_slice(&mut self, other: &[u8]) {
        self.parser.extend_from_slice(other)
    }

    /// Tell the parser that the file is finished.
    ///
    /// This triggers the parsing of the final bytes and might lead [`read_next`](Self::read_next) to return some extra values.
    pub fn end(&mut self) {
        self.parser.end()
    }

    /// Returns if the parsing is finished i.e. [`end`](Self::end) has been called and [`read_next`](Self::read_next) is always going to return `None`.
    pub fn is_end(&self) -> bool {
        self.parser.is_end()
    }

    /// Attempt to parse a new quad from the already provided data.
    ///
    /// Returns [`None`] if the parsing is finished or more data is required.
    /// If it is the case more data should be fed using [`extend_from_slice`](Self::extend_from_slice).
    pub fn read_next(&mut self) -> Option<Result<N3Quad, SyntaxError>> {
        self.parser.read_next()
    }
}

#[derive(Clone)]
enum Predicate {
    Regular(N3Term),
    Inverted(N3Term),
}

struct N3Recognizer {
    stack: Vec<N3State>,
    lexer_options: N3LexerOptions,
    prefixes: HashMap<String, Iri<String>>,
    terms: Vec<N3Term>,
    predicates: Vec<Predicate>,
    contexts: Vec<BlankNode>,
}

impl RuleRecognizer for N3Recognizer {
    type TokenRecognizer = N3Lexer;
    type Output = N3Quad;

    fn error_recovery_state(mut self) -> Self {
        self.stack.clear();
        self.terms.clear();
        self.predicates.clear();
        self.contexts.clear();
        self
    }

    fn recognize_next(
        mut self,
        token: N3Token,
        results: &mut Vec<N3Quad>,
        errors: &mut Vec<RuleRecognizerError>,
    ) -> Self {
        if let Some(rule) = self.stack.pop() {
            match rule {
                // [1] 	n3Doc 	::= 	( ( n3Statement ".") | sparqlDirective) *
                // [2] 	n3Statement 	::= 	n3Directive | triples
                // [3] 	n3Directive 	::= 	prefixID | base
                // [4] 	sparqlDirective 	::= 	sparqlBase | sparqlPrefix
                // [5] 	sparqlBase 	::= 	BASE IRIREF
                // [6] 	sparqlPrefix 	::= 	PREFIX PNAME_NS IRIREF
                // [7] 	prefixID 	::= 	"@prefix" PNAME_NS IRIREF
                // [8] 	base 	::= 	"@base" IRIREF
                N3State::N3Doc => {
                    self.stack.push(N3State::N3Doc);
                    match token {
                        N3Token::PlainKeyword(k) if k.eq_ignore_ascii_case("base") => {
                            self.stack.push(N3State::BaseExpectIri);
                            self
                        }
                        N3Token::PlainKeyword(k) if k.eq_ignore_ascii_case("prefix") => {
                            self.stack.push(N3State::PrefixExpectPrefix);
                            self
                        }
                        N3Token::LangTag("prefix") => {
                            self.stack.push(N3State::N3DocExpectDot);
                            self.stack.push(N3State::PrefixExpectPrefix);
                            self
                        }
                        N3Token::LangTag("base") => {
                            self.stack.push(N3State::N3DocExpectDot);
                            self.stack.push(N3State::BaseExpectIri);
                            self
                        }
                        token => {
                            self.stack.push(N3State::N3DocExpectDot);
                            self.stack.push(N3State::Triples);
                            self.recognize_next(token, results, errors)
                        }
                    }
                },
                N3State::N3DocExpectDot => {
                    if token == N3Token::Punctuation(".") {
                        self
                    } else  {
                        errors.push("A dot is expected at the end of N3 statements".into());
                        self.recognize_next(token, results, errors)
                    }
                },
                N3State::BaseExpectIri => match token {
                    N3Token::IriRef(iri) => {
                        self.lexer_options.base_iri = Some(iri);
                        self
                    }
                    _ => self.error(errors, "The BASE keyword should be followed by an IRI"),
                },
                N3State::PrefixExpectPrefix => match token {
                    N3Token::PrefixedName { prefix, local, .. } if local.is_empty() => {
                        self.stack.push(N3State::PrefixExpectIri { name: prefix.to_owned() });
                        self
                    }
                    _ => {
                        self.error(errors, "The PREFIX keyword should be followed by a prefix like 'ex:'")
                    }
                },
                N3State::PrefixExpectIri { name } => match token {
                    N3Token::IriRef(iri) => {
                        self.prefixes.insert(name, iri);
                        self
                    }
                    _ => self.error(errors, "The PREFIX declaration should be followed by a prefix and its value as an IRI"),
                },
                // [9] 	triples 	::= 	subject predicateObjectList?
                N3State::Triples => {
                    self.stack.push(N3State::TriplesMiddle);
                    self.stack.push(N3State::Path);
                    self.recognize_next(token, results, errors)
                },
                N3State::TriplesMiddle => if matches!(token, N3Token::Punctuation("." | "]" | "}" | ")")) {
                    self.recognize_next(token, results, errors)
                } else {
                    self.stack.push(N3State::TriplesEnd);
                    self.stack.push(N3State::PredicateObjectList);
                    self.recognize_next(token, results, errors)
                },
                N3State::TriplesEnd => {
                    self.terms.pop();
                    self.recognize_next(token, results, errors)
                },
                // [10] 	predicateObjectList 	::= 	verb objectList ( ";" ( verb objectList) ? ) *
                N3State::PredicateObjectList => {
                    self.stack.push(N3State::PredicateObjectListEnd);
                    self.stack.push(N3State::ObjectsList);
                    self.stack.push(N3State::Verb);
                    self.recognize_next(token, results, errors)
                },
                N3State::PredicateObjectListEnd => {
                    self.predicates.pop();
                    if token == N3Token::Punctuation(";") {
                        self.stack.push(N3State::PredicateObjectListPossibleContinuation);
                        self
                    } else {
                        self.recognize_next(token, results, errors)
                    }
                },
                N3State::PredicateObjectListPossibleContinuation => if token == N3Token::Punctuation(";") {
                    self.stack.push(N3State::PredicateObjectListPossibleContinuation);
                    self
                } else if matches!(token, N3Token::Punctuation(";" | "." | "}" | "]" | ")")) {
                    self.recognize_next(token, results, errors)
                } else {
                    self.stack.push(N3State::PredicateObjectListEnd);
                    self.stack.push(N3State::ObjectsList);
                    self.stack.push(N3State::Verb);
                    self.recognize_next(token, results, errors)
                },
                // [11] 	objectList 	::= 	object ( "," object) *
                N3State::ObjectsList => {
                    self.stack.push(N3State::ObjectsListEnd);
                    self.stack.push(N3State::Path);
                    self.recognize_next(token, results, errors)
                }
                N3State::ObjectsListEnd => {
                    let object = self.terms.pop().unwrap();
                    let subject = self.terms.last().unwrap().clone();
                    results.push(match self.predicates.last().unwrap().clone() {
                        Predicate::Regular(predicate) => self.quad(
                            subject,
                            predicate,
                            object,
                        ),
                        Predicate::Inverted(predicate) => self.quad(
                            object,
                            predicate,
                            subject,
                        )
                    });
                    if token == N3Token::Punctuation(",") {
                        self.stack.push(N3State::ObjectsListEnd);
                        self.stack.push(N3State::Path);
                        self
                    } else {
                        self.recognize_next(token, results, errors)
                    }
                },
                // [12] 	verb 	::= 	predicate | "a" | ( "has" expression) | ( "is" expression "of") | "=" | "<=" | "=>"
                // [14] 	predicate 	::= 	expression | ( "<-" expression)
                N3State::Verb => match token {
                    N3Token::PlainKeyword("a") => {
                        self.predicates.push(Predicate::Regular(rdf::TYPE.into()));
                        self
                    }
                    N3Token::PlainKeyword("has") => {
                        self.stack.push(N3State::AfterRegularVerb);
                        self.stack.push(N3State::Path);
                        self
                    }
                    N3Token::PlainKeyword("is") => {
                        self.stack.push(N3State::AfterVerbIs);
                        self.stack.push(N3State::Path);
                        self
                    }
                    N3Token::Punctuation("=") => {
                        self.predicates.push(Predicate::Regular(NamedNode::new_unchecked("http://www.w3.org/2002/07/owl#sameAs").into()));
                        self
                    }
                    N3Token::Punctuation("=>") => {
                        self.predicates.push(Predicate::Regular(NamedNode::new_unchecked("http://www.w3.org/2000/10/swap/log#implies").into()));
                        self
                    }
                    N3Token::Punctuation("<=") => {
                        self.predicates.push(Predicate::Inverted(NamedNode::new_unchecked("http://www.w3.org/2000/10/swap/log#implies").into()));
                        self
                    }
                    N3Token::Punctuation("<-") => {
                        self.stack.push(N3State::AfterInvertedVerb);
                        self.stack.push(N3State::Path);
                        self
                    }
                    token => {
                        self.stack.push(N3State::AfterRegularVerb);
                        self.stack.push(N3State::Path);
                        self.recognize_next(token, results, errors)
                    }
                }
                N3State::AfterRegularVerb => {
                    self.predicates.push(Predicate::Regular(self.terms.pop().unwrap()));
                    self.recognize_next(token, results, errors)
                }
                N3State::AfterInvertedVerb => {
                    self.predicates.push(Predicate::Inverted(self.terms.pop().unwrap()));
                    self.recognize_next(token, results, errors)
                }
                N3State::AfterVerbIs => match token {
                    N3Token::PlainKeyword("of") => {
                        self.predicates.push(Predicate::Inverted(self.terms.pop().unwrap()));
                        self
                    },
                    _ => {
                        self.error(errors, "The keyword 'is' should be followed by a predicate then by the keyword 'of'")
                    }
                }
                // [13] 	subject 	::= 	expression
                // [15] 	object 	::= 	expression
                // [16] 	expression 	::= 	path
                // [17] 	path 	::= 	pathItem ( ( "!" path) | ( "^" path) ) ?
                N3State::Path => {
                    self.stack.push(N3State::PathFollowUp);
                    self.stack.push(N3State::PathItem);
                    self.recognize_next(token, results, errors)
                }
                N3State::PathFollowUp => match token {
                    N3Token::Punctuation("!") => {
                        self.stack.push(N3State::PathAfterIndicator { is_inverse: false });
                        self.stack.push(N3State::PathItem);
                        self
                    }
                    N3Token::Punctuation("^") => {
                        self.stack.push(N3State::PathAfterIndicator { is_inverse: true });
                        self.stack.push(N3State::PathItem);
                        self
                    }
                    token => self.recognize_next(token, results, errors)
                },
                N3State::PathAfterIndicator { is_inverse } => {
                    let predicate = self.terms.pop().unwrap();
                    let previous = self.terms.pop().unwrap();
                    let current = BlankNode::default();
                    results.push(if is_inverse { self.quad(current.clone(), predicate, previous) } else { self.quad(previous, predicate, current.clone())});
                    self.terms.push(current.into());
                    self.stack.push(N3State::PathFollowUp);
                    self.recognize_next(token, results, errors)
                },
                // [18] 	pathItem 	::= 	iri | blankNode | quickVar | collection | blankNodePropertyList | iriPropertyList | literal | formula
                // [19] 	literal 	::= 	rdfLiteral | numericLiteral | BOOLEAN_LITERAL
                // [20] 	blankNodePropertyList 	::= 	"[" predicateObjectList "]"
                // [21] 	iriPropertyList 	::= 	IPLSTART iri predicateObjectList "]"
                // [22] 	collection 	::= 	"(" object* ")"
                // [23] 	formula 	::= 	"{" formulaContent? "}"
                // [25] 	numericLiteral 	::= 	DOUBLE | DECIMAL | INTEGER
                // [26] 	rdfLiteral 	::= 	STRING ( LANGTAG | ( "^^" iri) ) ?
                // [27] 	iri 	::= 	IRIREF | prefixedName
                // [28] 	prefixedName 	::= 	PNAME_LN | PNAME_NS
                // [29] 	blankNode 	::= 	BLANK_NODE_LABEL | ANON
                // [30] 	quickVar 	::= 	QUICK_VAR_NAME
                N3State::PathItem => {
                    match token {
                        N3Token::IriRef(iri) => {
                            self.terms.push(NamedNode::new_unchecked(iri.into_inner()).into());
                            self
                        }
                        N3Token::PrefixedName { prefix, local, might_be_invalid_iri } => match resolve_local_name(prefix, &local, might_be_invalid_iri, &self.prefixes) {
                            Ok(t) => {
                                self.terms.push(t.into());
                                self
                            },
                            Err(e) => self.error(errors, e)
                        }
                        N3Token::BlankNodeLabel(bnode) => {
                            self.terms.push(BlankNode::new_unchecked(bnode).into());
                            self
                        }
                        N3Token::Variable(name) => {
                            self.terms.push(Variable::new_unchecked(name).into());
                            self
                        }
                        N3Token::Punctuation("[") => {
                            self.stack.push(N3State::PropertyListMiddle);
                            self
                        }
                        N3Token::Punctuation("(") => {
                            self.stack.push(N3State::CollectionBeginning);
                            self
                        }
                        N3Token::String(value) => {
                            self.stack.push(N3State::LiteralPossibleSuffix { value });
                            self
                        }
                        N3Token::Integer(v) => {
                            self.terms.push(Literal::new_typed_literal(v, xsd::INTEGER).into());
                            self
                        }
                        N3Token::Decimal(v) => {
                            self.terms.push(Literal::new_typed_literal(v, xsd::DECIMAL).into());
                            self
                        }
                        N3Token::Double(v) => {
                            self.terms.push(Literal::new_typed_literal(v, xsd::DOUBLE).into());
                            self
                        }
                        N3Token::PlainKeyword("true") => {
                            self.terms.push(Literal::new_typed_literal("true", xsd::BOOLEAN).into());
                            self
                        }
                        N3Token::PlainKeyword("false") => {
                            self.terms.push(Literal::new_typed_literal("false", xsd::BOOLEAN).into());
                            self
                        }
                        N3Token::Punctuation("{") => {
                            self.contexts.push(BlankNode::default());
                            self.stack.push(N3State::FormulaContent);
                            self
                        }
                        token => self.error(errors, format!("This is not a valid RDF value: {token:?}"))
                    }
                }
                N3State::PropertyListMiddle => match token {
                    N3Token::Punctuation("]") => {
                        self.terms.push(BlankNode::default().into());
                        self
                    },
                    N3Token::PlainKeyword("id") => {
                        self.stack.push(N3State::IriPropertyList);
                        self
                    },
                    token => {
                        self.terms.push(BlankNode::default().into());
                        self.stack.push(N3State::PropertyListEnd);
                        self.stack.push(N3State::PredicateObjectList);
                        self.recognize_next(token, results, errors)
                    }
                }
                N3State::PropertyListEnd => if token == N3Token::Punctuation("]") {
                    self
                } else {
                    errors.push("blank node property lists should end with a ']'".into());
                    self.recognize_next(token, results, errors)
                }
                N3State::IriPropertyList => match token {
                    N3Token::IriRef(id) => {
                        self.terms.push(NamedNode::new_unchecked(id.into_inner()).into());
                        self.stack.push(N3State::PropertyListEnd);
                        self.stack.push(N3State::PredicateObjectList);
                        self
                    }
                    N3Token::PrefixedName { prefix, local, might_be_invalid_iri } => match resolve_local_name(prefix, &local, might_be_invalid_iri, &self.prefixes) {
                        Ok(t) => {
                            self.terms.push(t.into());
                            self.stack.push(N3State::PropertyListEnd);
                            self.stack.push(N3State::PredicateObjectList);
                            self
                        },
                        Err(e) => self.error(errors, e)
                    }
                    _ => {
                        self.error(errors, "The '[ id' construction should be followed by an IRI")
                    }
                }
                N3State::CollectionBeginning => match token {
                    N3Token::Punctuation(")") => {
                        self.terms.push(rdf::NIL.into());
                        self
                    }
                    token => {
                        let root = BlankNode::default();
                        self.terms.push(root.clone().into());
                        self.terms.push(root.into());
                        self.stack.push(N3State::CollectionPossibleEnd);
                        self.stack.push(N3State::Path);
                        self.recognize_next(token, results, errors)
                    }
                },
                N3State::CollectionPossibleEnd => {
                    let value = self.terms.pop().unwrap();
                    let old = self.terms.pop().unwrap();
                    results.push(self.quad(
                         old.clone(),
                         rdf::FIRST,
                         value,
                    ));
                    match token {
                        N3Token::Punctuation(")") => {
                            results.push(self.quad(old,
                                rdf::REST,
                                rdf::NIL
                            ));
                            self
                        }
                        token => {
                            let new = BlankNode::default();
                            results.push(self.quad( old,
                                rdf::REST,
                                new.clone()
                            ));
                            self.terms.push(new.into());
                            self.stack.push(N3State::CollectionPossibleEnd);
                            self.stack.push(N3State::Path);
                            self.recognize_next(token, results, errors)
                        }
                    }
                }
                N3State::LiteralPossibleSuffix { value } => {
                    match token {
                        N3Token::LangTag(lang) => {
                            self.terms.push(Literal::new_language_tagged_literal_unchecked(value, lang.to_ascii_lowercase()).into());
                            self
                        },
                        N3Token::Punctuation("^^") => {
                            self.stack.push(N3State::LiteralExpectDatatype { value });
                            self
                        }
                        token => {
                            self.terms.push(Literal::new_simple_literal(value).into());
                            self.recognize_next(token, results, errors)
                        }
                    }
                }
                N3State::LiteralExpectDatatype { value } => {
                    match token {
                        N3Token::IriRef(datatype) => {
                            self.terms.push(Literal::new_typed_literal(value, NamedNode::new_unchecked(datatype.into_inner())).into());
                            self
                        },
                        N3Token::PrefixedName { prefix, local, might_be_invalid_iri } => match resolve_local_name(prefix, &local, might_be_invalid_iri, &self.prefixes) {
                            Ok(datatype) =>{
                                self.terms.push(Literal::new_typed_literal(value, datatype).into());
                                self
                            },
                            Err(e) => self.error(errors, e)
                        }
                        token => {
                            self.error(errors, format!("Expecting a datatype IRI after '^^, found {token:?}")).recognize_next(token, results, errors)
                        }
                    }
                }
                // [24] 	formulaContent 	::= 	( n3Statement ( "." formulaContent? ) ? ) | ( sparqlDirective formulaContent? )
                N3State::FormulaContent => {
                    match token {
                        N3Token::Punctuation("}") => {
                            self.terms.push(self.contexts.pop().unwrap().into());
                            self
                        }
                        N3Token::PlainKeyword(k)if k.eq_ignore_ascii_case("base") => {
                            self.stack.push(N3State::FormulaContent);
                            self.stack.push(N3State::BaseExpectIri);
                            self
                        }
                        N3Token::PlainKeyword(k)if k.eq_ignore_ascii_case("prefix") => {
                            self.stack.push(N3State::FormulaContent);
                            self.stack.push(N3State::PrefixExpectPrefix);
                            self
                        }
                        N3Token::LangTag("prefix") => {
                            self.stack.push(N3State::FormulaContentExpectDot);
                            self.stack.push(N3State::PrefixExpectPrefix);
                            self
                        }
                        N3Token::LangTag("base") => {
                            self.stack.push(N3State::FormulaContentExpectDot);
                            self.stack.push(N3State::BaseExpectIri);
                            self
                        }
                        token => {
                            self.stack.push(N3State::FormulaContentExpectDot);
                            self.stack.push(N3State::Triples);
                            self.recognize_next(token, results, errors)
                        }
                    }
                }
                N3State::FormulaContentExpectDot => {
                    match token {
                        N3Token::Punctuation("}") => {
                            self.terms.push(self.contexts.pop().unwrap().into());
                            self
                        }
                        N3Token::Punctuation(".") => {
                            self.stack.push(N3State::FormulaContent);
                            self
                        }
                        token => {
                            errors.push("A dot is expected at the end of N3 statements".into());
                            self.stack.push(N3State::FormulaContent);
                            self.recognize_next(token, results, errors)
                        }
                    }
                }
            }
        } else if token == N3Token::Punctuation(".") {
            self.stack.push(N3State::N3Doc);
            self
        } else {
            self
        }
    }

    fn recognize_end(
        self,
        _results: &mut Vec<Self::Output>,
        errors: &mut Vec<RuleRecognizerError>,
    ) {
        match &*self.stack {
            [] | [N3State::N3Doc] => (),
            _ => errors.push("Unexpected end".into()), //TODO
        }
    }

    fn lexer_options(&self) -> &N3LexerOptions {
        &self.lexer_options
    }
}

impl N3Recognizer {
    pub fn new_parser(
        base_iri: Option<Iri<String>>,
        prefixes: HashMap<String, Iri<String>>,
    ) -> Parser<Self> {
        Parser::new(
            Lexer::new(
                N3Lexer::new(N3LexerMode::N3),
                MIN_BUFFER_SIZE,
                MAX_BUFFER_SIZE,
                true,
                Some(b"#"),
            ),
            N3Recognizer {
                stack: vec![N3State::N3Doc],
                lexer_options: N3LexerOptions { base_iri },
                prefixes,
                terms: Vec::new(),
                predicates: Vec::new(),
                contexts: Vec::new(),
            },
        )
    }

    #[must_use]
    fn error(
        mut self,
        errors: &mut Vec<RuleRecognizerError>,
        msg: impl Into<RuleRecognizerError>,
    ) -> Self {
        errors.push(msg.into());
        self.stack.clear();
        self
    }

    fn quad(
        &self,
        subject: impl Into<N3Term>,
        predicate: impl Into<N3Term>,
        object: impl Into<N3Term>,
    ) -> N3Quad {
        N3Quad {
            subject: subject.into(),
            predicate: predicate.into(),
            object: object.into(),
            graph_name: self
                .contexts
                .last()
                .map_or(GraphName::DefaultGraph, |g| g.clone().into()),
        }
    }
}

#[derive(Debug)]
enum N3State {
    N3Doc,
    N3DocExpectDot,
    BaseExpectIri,
    PrefixExpectPrefix,
    PrefixExpectIri { name: String },
    Triples,
    TriplesMiddle,
    TriplesEnd,
    PredicateObjectList,
    PredicateObjectListEnd,
    PredicateObjectListPossibleContinuation,
    ObjectsList,
    ObjectsListEnd,
    Verb,
    AfterRegularVerb,
    AfterInvertedVerb,
    AfterVerbIs,
    Path,
    PathFollowUp,
    PathAfterIndicator { is_inverse: bool },
    PathItem,
    PropertyListMiddle,
    PropertyListEnd,
    IriPropertyList,
    CollectionBeginning,
    CollectionPossibleEnd,
    LiteralPossibleSuffix { value: String },
    LiteralExpectDatatype { value: String },
    FormulaContent,
    FormulaContentExpectDot,
}
