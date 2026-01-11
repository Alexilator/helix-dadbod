(require "helix/components.scm")
(require "helix/misc.scm")
(require "helix/editor.scm")
(require (prefix-in helix. "helix/commands.scm"))
(require (only-in "helix/static.scm" jump_view_up jump_view_down))

;; Load helix-dadbod FFI library
;; Note: Functions use Dadbod:: prefix from Rust registration
(#%require-dylib "libhelix_dadbod"
    (only-in
        Dadbod::list_connections
        Dadbod::connect
        Dadbod::test_connection
        Dadbod::execute_query
        Dadbod::close_connection
        Dadbod::get_workspace_path
        Dadbod::get_init_error
        WorkspaceInfo-path
        WorkspaceInfo-sql_file
        WorkspaceInfo-dbout_file))


;;; Database Connection Picker for Helix
;;; Shows a popup listing connections from ~/.config/helix-dadbod/config.toml

;;; ============================================================================
;;; Auto-Execute on Save
;;; ============================================================================

;; Helper: Extract connection name from SQL file path
;; Path format: /tmp/helix-dadbod/{connection_name}.sql
(define (extract-connection-name path)
  (define prefix "/tmp/helix-dadbod/")
  (define suffix ".sql")
  (if (and (starts-with? path prefix)
           (ends-with? path suffix))
      (let* ([after-prefix (substring path (string-length prefix))]
             [conn-name (substring after-prefix 0 (- (string-length after-prefix) (string-length suffix)))])
        conn-name)
      #f))

;; Helper: Find and reload the shared dbout file
;; This function switches to the dbout buffer, reloads it, then switches back
(define (reload-dbout-file connection-name)
  (define dbout-path "/tmp/helix-dadbod/results.dbout")
  (define sql-path (string-append "/tmp/helix-dadbod/" connection-name ".sql"))

  ;; Check if dbout file is open in any buffer
  (define all-docs (editor-all-documents))
  (define dbout-is-open #f)

  (for-each
    (lambda (doc-id)
      (let ([doc-path (editor-document->path doc-id)])
        (when (and doc-path (equal? doc-path dbout-path))
          (set! dbout-is-open #t))))
    all-docs)

  ;; If the dbout file is open, switch to it, reload, then switch back
  (when dbout-is-open
    ;; Open dbout file (switches to it if already open)
    (helix.open dbout-path)
    ;; Reload the current buffer
    (helix.reload)
    ;; Switch back to the SQL file
    (helix.open sql-path)))

;; Helper: Check if current file is a SQL file and execute if so
(define (maybe-execute-query)
  (define focus (editor-focus))
  (define doc-id (editor->doc-id focus))
  (define path (editor-document->path doc-id))

  (if (not path)
      (begin
        (set-status! "No path found for current document")
        void)
      (let ([conn-name (extract-connection-name path)])
        (if (not conn-name)
            (begin
              (set-status! (string-append "Not a SQL file: " path))
              void)
            ;; This is a SQL file - execute the query
            (begin
              (Dadbod::execute_query conn-name)
              (reload-dbout-file conn-name)
              (set-status! (string-append "Query executed: " conn-name))
              void)))))

;;@doc
;; Execute the current SQL query (without saving)
(define (db-execute)
  (maybe-execute-query))

;;; ============================================================================
;;; Connection Data Functions
;;; ============================================================================

;; Get list of connection names from Rust library
;; Rust handles all TOML parsing, file I/O, and error handling
(define (get-connection-names)
  (Dadbod::list_connections))

;; Open workspace files in horizontal split
;; If results.dbout is already open, only open the SQL file in upper split
;; Otherwise, open SQL file in current view and results.dbout in hsplit below
(define (open-workspace-files workspace)
  (define sql-file (WorkspaceInfo-sql_file workspace))
  (define dbout-file (WorkspaceInfo-dbout_file workspace))

  ;; Check if dbout is already open
  (define all-docs (editor-all-documents))
  (define dbout-already-open #f)

  (for-each
    (lambda (doc-id)
      (let ([doc-path (editor-document->path doc-id)])
        (when (and doc-path (equal? doc-path dbout-file))
          (set! dbout-already-open #t))))
    all-docs)

  (if dbout-already-open
      ;; Dbout is already open, just open the SQL file in upper split
      (begin
        ;; Make sure we're in the upper split by jumping up
        (jump_view_up)
        ;; Open the SQL file (will replace current buffer in upper split)
        (helix.open sql-file))
      ;; First connection: open both files in hsplit layout
      (begin
        ;; Open SQL file in current view
        (helix.open sql-file)
        ;; Open results.dbout in horizontal split below
        (helix.hsplit dbout-file)
        ;; Focus back on SQL file (upper split)
        (jump_view_up))))

;;; ============================================================================
;;; Component State
;;; ============================================================================

;; State struct for the database picker component
(struct DadbodState (connections selected-index) #:mutable)

;; Global state storage for selected connection
(define *selected-connection* #f)

(define (db-get-connection)
  *selected-connection*)

(define (set-selected-connection! name)
  (set! *selected-connection* name))

;;; ============================================================================
;;; Render Function
;;; ============================================================================

(define (render-dadbod-picker state rect buffer)
  (let* ([connections (DadbodState-connections state)]
         [selected-idx (DadbodState-selected-index state)]
         [num-connections (length connections)]

         ;; Calculate popup dimensions
         [popup-width (min 60 (- (area-width rect) 4))]
         [popup-height (min (+ num-connections 4) (- (area-height rect) 4))]
         [popup-x (ceiling (- (/ (area-width rect) 2) (/ popup-width 2)))]
         [popup-y (ceiling (- (/ (area-height rect) 2) (/ popup-height 2)))]
         [popup-area (area popup-x popup-y popup-width popup-height)]

         ;; Get theme styles
         [popup-style (theme-scope "ui.popup")]
         [selected-style (theme-scope "ui.text.focus")]
         [number-style (theme-scope "markup.list")])

    ;; Clear buffer and render popup block
    (buffer/clear buffer popup-area)
    (block/render buffer popup-area (make-block popup-style (style) "all" "plain"))

    ; Render title
    (frame-set-string! buffer (+ popup-x 2) (+ popup-y 0)
                       "Database Connections" popup-style)

    ;; Render connection list
    (for-each
      (lambda (i)
        (let* ([conn-name (list-ref connections i)]
               [is-selected (= i selected-idx)]
               [item-style (if is-selected selected-style popup-style)]
               [y-pos (+ popup-y 2 i)]
               [row-area (area (+ popup-x 1) y-pos (- popup-width 2) 1)])
          ;; Fill background for selected row
          (when is-selected
            (buffer/clear-with buffer row-area selected-style))
          ;; Render number (1-9, or blank for 10+)
          (when (< i 9)
            (frame-set-string! buffer (+ popup-x 2) y-pos
                               (number->string (+ i 1)) item-style))
          ;; Render connection name
          (frame-set-string! buffer (+ popup-x 5) y-pos conn-name item-style)))
      (range num-connections))

    ;; Render help text at bottom
    (frame-set-string! buffer (+ popup-x 2) (+ popup-y popup-height -1)
                       "1-9/Enter: Select  Tab/↑↓: Navigate  Esc: Close"
                       popup-style)))

;;; ============================================================================
;;; Event Handler
;;; ============================================================================

(define (handle-dadbod-event state event)
  (let* ([connections (DadbodState-connections state)]
         [num-connections (length connections)]
         [current-idx (DadbodState-selected-index state)]
         [char (key-event-char event)])

    (cond
      ;; Close popup: Escape or 'q'
      [(or (key-event-escape? event) (eqv? char #\q))
       event-result/close]

      ;; Navigate down: Arrow down, 'j', or Tab (without shift)
      [(or (key-event-down? event) 
           (eqv? char #\j)
           (and (key-event-tab? event)
                (not (equal? (key-event-modifier event) key-modifier-shift))))
       (set-DadbodState-selected-index! state
         (modulo (+ current-idx 1) num-connections))
       event-result/consume]

      ;; Navigate up: Arrow up, 'k', or Shift+Tab
      [(or (key-event-up? event) 
           (eqv? char #\k)
           (and (key-event-tab? event)
                (equal? (key-event-modifier event) key-modifier-shift)))
       (set-DadbodState-selected-index! state
         (modulo (- current-idx 1) num-connections))
       event-result/consume]

      ;; Select with Enter
      [(key-event-enter? event)
       (let ([selected-name (list-ref connections current-idx)])
         (define workspace (Dadbod::connect selected-name))
         (if workspace
             (begin
               (set-selected-connection! selected-name)
               (set-status! (string-append "Connected: " selected-name
                                           " → SQL: " (WorkspaceInfo-sql_file workspace)))
               ;; Open workspace files in hsplit
               (open-workspace-files workspace)
               event-result/close)
             (begin
               (set-error! (string-append "Failed to connect to: " selected-name
                                          ". Check ~/.config/helix-dadbod/dadbod.log for details"))
               event-result/close)))]

      ;; Select by number (1-9)
      [(and char (char-digit? char))
       (let* ([num (char->number char)]
              [idx (- num 1)])
         (if (and (>= idx 0) (< idx num-connections))
             (let ([selected-name (list-ref connections idx)])
               (define workspace (Dadbod::connect selected-name))
               (if workspace
                   (begin
                     (set-selected-connection! selected-name)
                     (set-status! (string-append "Connected: " selected-name
                                                 " → SQL: " (WorkspaceInfo-sql_file workspace)))
                     ;; Open workspace files in hsplit
                     (open-workspace-files workspace)
                     event-result/close)
                   (begin
                     (set-error! (string-append "Failed to connect to: " selected-name
                                                ". Check ~/.config/helix-dadbod/dadbod.log for details"))
                     event-result/close)))
             event-result/consume))]

      ;; Ignore other events
      [else event-result/ignore])))

;;; ============================================================================
;;; Main Entry Point
;;; ============================================================================

;;@doc
;; Open the database connection picker popup
(define (db-open-picker)
  ;; First check if initialization succeeded
  (define init-error (Dadbod::get_init_error))
  (if (not (equal? init-error ""))
      ;; Initialization failed - show error
      (set-error! (string-append "config.toml is malformed. Check ~/.config/helix-dadbod/dadbod.log for details"))
      ;; Initialization succeeded - show picker
      (let* ([connections (get-connection-names)])
        (if (null? connections)
            (set-error! "No connections found in ~/.config/helix-dadbod/config.toml")
            (let ([component (new-component!
                              "dadbod-picker"
                              (DadbodState connections 0)
                              render-dadbod-picker
                              (hash "handle_event" handle-dadbod-event))])
              (push-component! component))))))

;;; ============================================================================
;;; Command Aliases
;;; ============================================================================

;;@doc
;; Short alias for db-execute
(define (dbe)
  (db-execute))

;;; ============================================================================
;;; Exports
;;; ============================================================================

(provide db-open-picker db-get-connection db-execute dbe)
