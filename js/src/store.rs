#![allow(clippy::use_self)]

use crate::format_err;
use crate::model::*;
use crate::utils::to_err;
use js_sys::{Array, Map};
use oxigraph::io::RdfFormat;
use oxigraph::model::*;
use oxigraph::sparql::QueryResults;
use oxigraph::store::Store;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = Store)]
pub struct JsStore {
    store: Store,
}

#[wasm_bindgen(js_class = Store)]
impl JsStore {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::use_self)]
    pub fn new(quads: Option<Box<[JsValue]>>) -> Result<JsStore, JsValue> {
        console_error_panic_hook::set_once();

        let store = Self {
            store: Store::new().map_err(to_err)?,
        };
        if let Some(quads) = quads {
            for quad in &*quads {
                store.add(quad)?;
            }
        }
        Ok(store)
    }

    pub fn add(&self, quad: &JsValue) -> Result<(), JsValue> {
        self.store
            .insert(&FROM_JS.with(|c| c.to_quad(quad))?)
            .map_err(to_err)?;
        Ok(())
    }

    pub fn delete(&self, quad: &JsValue) -> Result<(), JsValue> {
        self.store
            .remove(&FROM_JS.with(|c| c.to_quad(quad))?)
            .map_err(to_err)?;
        Ok(())
    }

    pub fn has(&self, quad: &JsValue) -> Result<bool, JsValue> {
        self.store
            .contains(&FROM_JS.with(|c| c.to_quad(quad))?)
            .map_err(to_err)
    }

    #[wasm_bindgen(getter=size)]
    pub fn size(&self) -> Result<usize, JsValue> {
        self.store.len().map_err(to_err)
    }

    #[wasm_bindgen(js_name = match)]
    pub fn match_quads(
        &self,
        subject: &JsValue,
        predicate: &JsValue,
        object: &JsValue,
        graph_name: &JsValue,
    ) -> Result<Box<[JsValue]>, JsValue> {
        Ok(self
            .store
            .quads_for_pattern(
                if let Some(subject) = FROM_JS.with(|c| c.to_optional_term(subject))? {
                    Some(subject.try_into()?)
                } else {
                    None
                }
                .as_ref()
                .map(<&Subject>::into),
                if let Some(predicate) = FROM_JS.with(|c| c.to_optional_term(predicate))? {
                    Some(NamedNode::try_from(predicate)?)
                } else {
                    None
                }
                .as_ref()
                .map(<&NamedNode>::into),
                if let Some(object) = FROM_JS.with(|c| c.to_optional_term(object))? {
                    Some(object.try_into()?)
                } else {
                    None
                }
                .as_ref()
                .map(<&Term>::into),
                if let Some(graph_name) = FROM_JS.with(|c| c.to_optional_term(graph_name))? {
                    Some(graph_name.try_into()?)
                } else {
                    None
                }
                .as_ref()
                .map(<&GraphName>::into),
            )
            .map(|v| v.map(|v| JsQuad::from(v).into()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(to_err)?
            .into_boxed_slice())
    }

    pub fn query(&self, query: &str) -> Result<JsValue, JsValue> {
        let results = self.store.query(query).map_err(to_err)?;
        let output = match results {
            QueryResults::Solutions(solutions) => {
                let results = Array::new();
                for solution in solutions {
                    let solution = solution.map_err(to_err)?;
                    let result = Map::new();
                    for (variable, value) in solution.iter() {
                        result.set(
                            &variable.as_str().into(),
                            &JsTerm::from(value.clone()).into(),
                        );
                    }
                    results.push(&result.into());
                }
                results.into()
            }
            QueryResults::Graph(quads) => {
                let results = Array::new();
                for quad in quads {
                    results.push(
                        &JsQuad::from(quad.map_err(to_err)?.in_graph(GraphName::DefaultGraph))
                            .into(),
                    );
                }
                results.into()
            }
            QueryResults::Boolean(b) => b.into(),
        };
        Ok(output)
    }

    pub fn update(&self, update: &str) -> Result<(), JsValue> {
        self.store.update(update).map_err(to_err)
    }

    pub fn load(
        &self,
        data: &str,
        mime_type: &str,
        base_iri: &JsValue,
        to_graph_name: &JsValue,
    ) -> Result<(), JsValue> {
        let Some(format) = RdfFormat::from_media_type(mime_type) else {
            return Err(format_err!("Not supported MIME type: {mime_type}"));
        };
        let base_iri = if base_iri.is_null() || base_iri.is_undefined() {
            None
        } else if base_iri.is_string() {
            base_iri.as_string()
        } else if let JsTerm::NamedNode(base_iri) = FROM_JS.with(|c| c.to_term(base_iri))? {
            Some(base_iri.value())
        } else {
            return Err(format_err!(
                "If provided, the base IRI should be a NamedNode or a string"
            ));
        };

        if let Some(to_graph_name) = FROM_JS.with(|c| c.to_optional_term(to_graph_name))? {
            self.store.load_graph(
                data.as_bytes(),
                format,
                GraphName::try_from(to_graph_name)?,
                base_iri.as_deref(),
            )
        } else {
            self.store
                .load_dataset(data.as_bytes(), format, base_iri.as_deref())
        }
        .map_err(to_err)
    }

    pub fn dump(&self, mime_type: &str, from_graph_name: &JsValue) -> Result<String, JsValue> {
        let Some(format) = RdfFormat::from_media_type(mime_type) else {
            return Err(format_err!("Not supported MIME type: {mime_type}"));
        };
        let mut buffer = Vec::new();
        if let Some(from_graph_name) = FROM_JS.with(|c| c.to_optional_term(from_graph_name))? {
            self.store
                .dump_graph(&mut buffer, format, &GraphName::try_from(from_graph_name)?)
        } else {
            self.store.dump_dataset(&mut buffer, format)
        }
        .map_err(to_err)?;
        String::from_utf8(buffer).map_err(to_err)
    }
}
