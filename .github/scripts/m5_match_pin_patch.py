from pathlib import Path


def replace_once(path: str, old: str, new: str, label: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    file.write_text(text.replace(old, new))


replace_once(
    "src/query_state.rs",
    '''    FileIdentity, MAX_SCAN_BYTES, MAX_SCAN_KEYWORD_CHARS, MAX_SCAN_RESULTS, QuerySummary,\n    SourceFileSnapshot,\n};''',
    '''    FileIdentity, GenerationPin, MAX_SCAN_BYTES, MAX_SCAN_KEYWORD_CHARS, MAX_SCAN_RESULTS,\n    QuerySummary, SourceFileSnapshot,\n};''',
    "query state generation pin import",
)

replace_once(
    "src/query_state.rs",
    '''    pub fn insert(&self, data: MatchReferenceData) -> Result<String, QueryStateError> {\n        data.validate()?;\n        let now = Instant::now();\n        let expires_at = expiration(now, self.ttl)?;\n        let mut state = self.lock_state();\n        state.purge_expired(now);\n        while state.entries.len() >= self.capacity {\n            state.evict_oldest();\n        }\n        let token = unique_token(MATCH_REFERENCE_PREFIX, &state.entries);\n        state.order.push_back(token.clone());\n        state\n            .entries\n            .insert(token.clone(), StoredMatch { data, expires_at });\n        Ok(token)\n    }\n\n    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {''',
    '''    pub fn insert(&self, data: MatchReferenceData) -> Result<String, QueryStateError> {\n        self.insert_with_pin(data, None)\n    }\n\n    pub fn insert_with_pin(\n        &self,\n        data: MatchReferenceData,\n        generation_pin: Option<GenerationPin>,\n    ) -> Result<String, QueryStateError> {\n        data.validate()?;\n        let now = Instant::now();\n        let expires_at = expiration(now, self.ttl)?;\n        let mut state = self.lock_state();\n        state.purge_expired(now);\n        while state.entries.len() >= self.capacity {\n            state.evict_oldest();\n        }\n        let token = unique_token(MATCH_REFERENCE_PREFIX, &state.entries);\n        state.order.push_back(token.clone());\n        state.entries.insert(\n            token.clone(),\n            StoredMatch {\n                data,\n                generation_pin,\n                expires_at,\n            },\n        );\n        Ok(token)\n    }\n\n    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {''',
    "match store insert with pin",
)

replace_once(
    "src/query_state.rs",
    '''    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {\n        validate_token(token, MATCH_REFERENCE_PREFIX)?;\n        let now = Instant::now();\n        let mut state = self.lock_state();\n        state.purge_expired(now);\n        state\n            .entries\n            .get(token)\n            .map(|entry| entry.data.clone())\n            .ok_or(QueryStateError::UnknownOrExpired)\n    }\n\n    #[must_use]\n    pub fn len(&self) -> usize {''',
    '''    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {\n        self.resolve_with_pin(token).map(|(data, _)| data)\n    }\n\n    pub fn resolve_with_pin(\n        &self,\n        token: &str,\n    ) -> Result<(MatchReferenceData, Option<GenerationPin>), QueryStateError> {\n        validate_token(token, MATCH_REFERENCE_PREFIX)?;\n        let now = Instant::now();\n        let mut state = self.lock_state();\n        state.purge_expired(now);\n        state\n            .entries\n            .get(token)\n            .map(|entry| (entry.data.clone(), entry.generation_pin.clone()))\n            .ok_or(QueryStateError::UnknownOrExpired)\n    }\n\n    #[must_use]\n    pub fn len(&self) -> usize {''',
    "match store resolve with pin",
)

replace_once(
    "src/query_state.rs",
    '''struct StoredMatch {\n    data: MatchReferenceData,\n    expires_at: Instant,\n}''',
    '''struct StoredMatch {\n    data: MatchReferenceData,\n    generation_pin: Option<GenerationPin>,\n    expires_at: Instant,\n}''',
    "stored match generation pin",
)

replace_once(
    "src/stateful_query.rs",
    '''    MAX_SCAN_RESULTS, MatchReferenceData, MatchReferenceStore, QueryBinding, QueryMatch,\n    QueryPageStopReason, QueryStateError, QuerySummary, ResultWatermark, ScanExecutor, ScanLimits,''',
    '''    GenerationPin, MAX_SCAN_RESULTS, MatchReferenceData, MatchReferenceStore, QueryBinding,\n    QueryMatch, QueryPageStopReason, QueryStateError, QuerySummary, ResultWatermark, ScanExecutor,\n    ScanLimits,''',
    "stateful query generation pin import",
)

replace_once(
    "src/stateful_query.rs",
    '''            let match_ref = self\n                .match_references\n                .insert(result.match_reference.clone())?;''',
    '''            let match_ref = self.match_references.insert_with_pin(\n                result.match_reference.clone(),\n                result.generation_pin.clone(),\n            )?;''',
    "register match ref with pin",
)

replace_once(
    "src/stateful_query.rs",
    '''struct RankedRegisteredMatch {\n    key: ResultWatermark,\n    value: QueryMatch,\n    match_reference: MatchReferenceData,\n}''',
    '''struct RankedRegisteredMatch {\n    key: ResultWatermark,\n    value: QueryMatch,\n    match_reference: MatchReferenceData,\n    generation_pin: Option<GenerationPin>,\n}''',
    "ranked match pin",
)

replace_once(
    "src/stateful_query.rs",
    '''            },\n            match_reference,\n        });''',
    '''            },\n            match_reference,\n            generation_pin: candidate.snapshot.generation_pin().cloned(),\n        });''',
    "copy candidate pin into match",
)

replace_once(
    "src/stateful_context.rs",
    '''        let reference = self.match_references.resolve(&request.match_ref)?;''',
    '''        let (reference, generation_pin) =\n            self.match_references.resolve_with_pin(&request.match_ref)?;''',
    "context resolve with pin",
)

replace_once(
    "src/stateful_context.rs",
    '''            .executor\n            .execute(cancellation, Some(deadline), move || {\n                read_referenced_context(''',
    '''            .executor\n            .execute(cancellation, Some(deadline), move || {\n                let _generation_pin = generation_pin;\n                read_referenced_context(''',
    "context worker holds pin",
)
