; zatara-netcoredbg — C# gutter runnables
;
; Contract (epic LOC-0103 business rules, DR-5):
;   * The tag strings `csharp-test-method` and `csharp-test-class` are the 3-way
;     join key between this query, tasks.json, and the DAP locator. Do not rename.
;   * Captures @csharp_namespace / @csharp_class_name / @csharp_method_name —
;     plus per-slot @csharp_case_arg_N / @csharp_case_raw_N /
;     @csharp_method_param_N on case rows — are exposed by Zed as
;     ${ZED_CUSTOM_...} task variables. The method/class lanes compose the
;     exact FullyQualifiedName= filter (never a bare name); case filters are
;     composed proxy-side from the captures (tasks.json: run-case CLI /
;     debug-scenario env keys).
;   * `_`-prefixed captures are match-only (Zed does not expose them).
;   * File-scoped namespaces: Zed's pinned grammar (tree-sitter-c-sharp @485f0ba)
;     parses `namespace X;` with members as SIBLINGS of the namespace node — the
;     `;` any-preceding-sibling operator below is what closes the reference
;     prototype's gap. `;` not `.`: a file-scoped namespace applies to the whole
;     file, so a comment (or any node) between `namespace X;` and the class must
;     not kill the icons — the `.` immediate-sibling anchor did exactly that
;     (owner-confirmed live 2026-10-21: deleting the comment restored them).
;   * Boundary: classes at the global namespace (no file-scoped or block
;     namespace) intentionally get no icons — an exact-FQN filter cannot be
;     composed from captures without a namespace, and a silently-wrong filter
;     is the defect class this epic kills. Global-namespace breadth is a
;     framework-breadth story (LOC-0113), not a guess here.

; ============================================================
; Test method (block namespace) — xunit / nunit / mstest
; ============================================================
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1) .))))))
  (#set! tag csharp-test-case-generic-1)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1) .)))))
  (#set! tag csharp-test-case-generic-1)
)

(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2) .))))))
  (#set! tag csharp-test-case-generic-2)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2) .)))))
  (#set! tag csharp-test-case-generic-2)
)

(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "InlineData")
                (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_3 @csharp_case_str_3) @csharp_case_raw_3)) (_ (_) @csharp_case_arg_3 @csharp_case_raw_3)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]") (#not-match? @csharp_case_arg_3 "^[@\"]")
              )
            name: (identifier) @csharp_method_name
            parameters: (parameter_list
              . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2)
              . (parameter name: (identifier) @csharp_method_param_3) .))))))
  (#set! tag csharp-test-case-generic-3)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "InlineData")
             (_ . [(_ ((string_literal (string_literal_content) @csharp_case_arg_1 @csharp_case_str_1) @csharp_case_raw_1)) (_ (_) @csharp_case_arg_1 @csharp_case_raw_1)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_2 @csharp_case_str_2) @csharp_case_raw_2)) (_ (_) @csharp_case_arg_2 @csharp_case_raw_2)] . [(_ ((string_literal (string_literal_content) @csharp_case_arg_3 @csharp_case_str_3) @csharp_case_raw_3)) (_ (_) @csharp_case_arg_3 @csharp_case_raw_3)] .) @run) (#not-match? @csharp_case_arg_1 "^[@\"]") (#not-match? @csharp_case_arg_2 "^[@\"]") (#not-match? @csharp_case_arg_3 "^[@\"]")
           )
         name: (identifier) @csharp_method_name
         parameters: (parameter_list
           . (parameter name: (identifier) @csharp_method_param_1)
              . (parameter name: (identifier) @csharp_method_param_2)
              . (parameter name: (identifier) @csharp_method_param_3) .)))))
  (#set! tag csharp-test-case-generic-3)
)
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "TestCase")
             (_
               (_ (integer_literal) @csharp_case_arg_1)
               (_ (integer_literal) @csharp_case_arg_2)
               (_ (integer_literal) @csharp_case_arg_3)) @run)
           )
         name: (identifier) @csharp_method_name))))
  (#set! tag csharp-test-case-nunit)
)

; ---- mstest [DataRow] --------------------------------------------------
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "DataRow")
                (_
                  (_ (integer_literal) @csharp_case_arg_1)
                  (_ (integer_literal) @csharp_case_arg_2)
                  (_ (integer_literal) @csharp_case_arg_3)) @run)
              )
            name: (identifier) @csharp_method_name)))))
  (#set! tag csharp-test-case-mstest)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (identifier) @_case_attr
             (#eq? @_case_attr "DataRow")
             (_
               (_ (integer_literal) @csharp_case_arg_1)
               (_ (integer_literal) @csharp_case_arg_2)
               (_ (integer_literal) @csharp_case_arg_3)) @run)
           )
         name: (identifier) @csharp_method_name))))
  (#set! tag csharp-test-case-mstest)
)

; ---- xunit [InlineData] -------------------------------------------------
; Also captures the method's formal parameter names: xunit's DisplayName
; names its arguments (`a: 1, b: 2`), so the case filter must zip
; parameters over arguments — the template does the zip, scm cannot.
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        (attribute_list
          (attribute
            name: (_) @class_attribute
            (#match? @class_attribute "^(TestFixture|TestClass)$")))
        name: (identifier) @run @csharp_class_name)))
  (#set! tag csharp-test-class)
)

; ============================================================
; Test method (block namespace) — attribute match
; ============================================================
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (_) @attribute_name
                (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))
            name: (identifier) @run @csharp_method_name)))))
  (#set! tag csharp-test-method)
)

; ============================================================
; Test method (file-scoped namespace)
; ============================================================
(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (_) @attribute_name
             (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))
         name: (identifier) @run @csharp_method_name))))
  (#set! tag csharp-test-method)
)

; ============================================================
; Parameterized cases — one gutter action per [TestCase]/[DataRow]/
; [InlineData] attribute, tagged by FRAMEWORK because each adapter's
; case filter grammar differs (live probes 2026-10-12, see tasks.json):
;   nunit   FQN includes the no-space arg body   -> FQN~body
;   mstest  FQN excludes args, Name is the row   -> Name~body
;   xunit   args only in the named DisplayName   -> DisplayName~p: v, …
; Arguments are captured one-by-one (nunit/mstest stay integer_literal-only
; at arity 3 — the committed fixture matrix); a case outside the captured
; shape gets NO case icon, never a guessed filter (same rule as the
; global-namespace boundary above). Method-level icons above still run all
; cases.
; The xunit [InlineData] lane is the per-arity generic pair above (tags
; `csharp-test-case-generic-1|2|3`, XUNIT-V3-MTP 2026-10-23): one pattern
; pair per arity replaces the per-shape family the owner's 2026-10-21
; ruling opened — that family died to Zed's hardcoded set_match_limit(64)
; (35 patterns rendered 71/96 icons at 5-class density; 15 render 96/96
; with headroom, offline-probed against Zed's pinned grammar rev). Each
; slot additionally captures its value AS WRITTEN (`@csharp_case_raw_N`:
; the string slot's quoted node, a primitive slot's bare node), so one RAW
; key exists per slot of the arity and the generic task templates never
; reference a capture a match lacks — Zed's hide law drops such templates
; from the menu silently.
; The string display body is `(p: "v")` and literal quotes cannot survive
; `dotnet test --filter`'s MSBuild-property round-trip (measured
; 2026-10-21: the echo shows them stripped), so the string lane selects by
; VALUE ONLY: `FQN~<method>& DisplayName~<v>` — a substring law (the space
; after `&` is deliberate for the static-text lanes: a spaceless AND'd
; filter reaches the Windows shell bare, where a naked `&` breaks the line
; — the run_lane_walker replay died on exactly that the first time; vstest
; trims around `&`), precise
; unless a sibling row's arg text contains this row's arg as a substring
; (recorded boundary, not a guessed filter).
; The xunit filters are composed proxy-side (zatara-proxy: the debug lane
; via build_filter, the plain-Run lane via the `run-case` CLI) — the quoted
; RAW value marks the string slot when no STR marker travels. nunit/mstest
; keep their composed static-text templates above.
; ============================================================

; ---- nunit [TestCase] -------------------------------------------------
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (identifier) @_case_attr
                (#eq? @_case_attr "TestCase")
                (_
                  (_ (integer_literal) @csharp_case_arg_1)
                  (_ (integer_literal) @csharp_case_arg_2)
                  (_ (integer_literal) @csharp_case_arg_3)) @run)
              )
            name: (identifier) @csharp_method_name)))))
  (#set! tag csharp-test-case-nunit)
)

; ============================================================
; Test class STRUCTURALLY: xunit has no class attribute, but a class
; CONTAINING a test-attributed method is a test class — the query can
; look inside the body and know it without any naming guess (owner
; ruling 2026-10-24: "a class has tests its a test class right?"; the
; convention-only law stranded e.g. `ShapeTests3`, a digit ending; the
; naming-convention pair was retired by this ruling — structural is the law).
; Namespace still required — the global-namespace boundary law above.
; ============================================================
(
  (namespace_declaration
    name: (_) @csharp_namespace
    body: (declaration_list
      (class_declaration
        name: (identifier) @run @csharp_class_name
        body: (declaration_list
          (method_declaration
            (attribute_list
              (attribute
                name: (_) @attribute_name
                (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$"))))))))
  (#set! tag csharp-test-class)
)

(
  ((file_scoped_namespace_declaration
     name: (_) @csharp_namespace)
   ;
   (class_declaration
     name: (identifier) @run @csharp_class_name
     body: (declaration_list
       (method_declaration
         (attribute_list
           (attribute
             name: (_) @attribute_name
             (#match? @attribute_name "^(Fact|Theory|Test|TestCase|TestCaseSource|TestMethod|DataTestMethod)$")))))))
  (#set! tag csharp-test-class)
)
