#include "rename.h"
#include "prism.h"
#include <stddef.h>
#include <stdlib.h>

typedef void (*sonicop_prism_diagnostic_callback)(void *, size_t, size_t, const char *, const char *);


typedef void (*sonicop_prism_regexp_callback)(void *, size_t, size_t, const uint8_t *, size_t,
                                            int, const uint8_t *, size_t, bool);
typedef struct {
    const pm_node_t **nodes;
    size_t size, capacity;
    bool failed;
} sonicop_node_stack;
static bool sonicop_stack_push_value(sonicop_node_stack *stack, const pm_node_t *node) {
    if (stack->size == stack->capacity) {
        size_t capacity = stack->capacity ? stack->capacity * 2 : 16;
        if (capacity < stack->capacity || capacity > SIZE_MAX / sizeof(*stack->nodes)) { stack->failed = true;return false; }
        const pm_node_t **nodes = realloc(stack->nodes, capacity * sizeof(*nodes));
        if (!nodes) { stack->failed = true;return false; }
        stack->nodes = nodes;stack->capacity = capacity;
    }
    stack->nodes[stack->size++] = node;return true;
}
static bool sonicop_stack_push(sonicop_node_stack *stack, const pm_node_t *node) {
    return !node || sonicop_stack_push_value(stack, node);
}
static void sonicop_push_list(sonicop_node_stack *stack, const pm_node_list_t *list) {
    for (size_t i = list->size; i > 0; i--) sonicop_stack_push(stack, list->nodes[i - 1]);
}
static bool sonicop_queue_child(const pm_node_t *node, void *data) {
    sonicop_stack_push(data, node);
    return false; /* pm_visit_node の再帰は使わず、呼び出し元の明示 stack へ積む。 */
}
/* Translation::Parser の builder.static_string が認める str と begin だけを平坦化する。 */
static bool sonicop_static_parts(sonicop_node_stack *pending, sonicop_node_stack *parts) {
    while (pending->size && !pending->failed) {
        const pm_node_t *node = pending->nodes[--pending->size];
        switch (PM_NODE_TYPE(node)) {
          case PM_STRING_NODE: sonicop_stack_push(parts, node);break;
          case PM_EMBEDDED_STATEMENTS_NODE: {
            const pm_statements_node_t *body = ((const pm_embedded_statements_node_t *)node)->statements;
            if (body) sonicop_stack_push(pending, (const pm_node_t *)body);
            else sonicop_stack_push_value(parts, NULL);
            break;
          }
          case PM_STATEMENTS_NODE: {
            const pm_node_list_t *body = &((const pm_statements_node_t *)node)->body;
            if (body->size) sonicop_push_list(pending, body);
            else sonicop_stack_push_value(parts, NULL);
            break;
          }
          case PM_PARENTHESES_NODE: {
            const pm_node_t *body = ((const pm_parentheses_node_t *)node)->body;
            if (body) sonicop_stack_push(pending, body);
            else sonicop_stack_push_value(parts, NULL);
            break;
          }
          case PM_INTERPOLATED_STRING_NODE: {
            const pm_node_list_t *children = &((const pm_interpolated_string_node_t *)node)->parts;
            if (children->size != 1 || PM_NODE_TYPE(children->nodes[0]) != PM_STRING_NODE) return false;
            sonicop_stack_push(pending, children->nodes[0]);break;
          }
          default: return false;
        }
    }
    return !pending->failed && !parts->failed;
}
static void sonicop_emit_regexp(const pm_node_t *node, const uint8_t *source, bool source_binary,
                               sonicop_prism_regexp_callback callback, void *data) {
    pm_location_t opening, closing;
    const pm_string_t *plain = NULL;
    const pm_node_list_t *children = NULL;
    switch (PM_NODE_TYPE(node)) {
      case PM_REGULAR_EXPRESSION_NODE: {
        const pm_regular_expression_node_t *literal = (const pm_regular_expression_node_t *)node;
        opening = literal->opening_loc;closing = literal->closing_loc;plain = &literal->unescaped;break;
      }
      case PM_MATCH_LAST_LINE_NODE: {
        const pm_match_last_line_node_t *literal = (const pm_match_last_line_node_t *)node;
        opening = literal->opening_loc;closing = literal->closing_loc;plain = &literal->unescaped;break;
      }
      case PM_INTERPOLATED_REGULAR_EXPRESSION_NODE: {
        const pm_interpolated_regular_expression_node_t *literal = (const pm_interpolated_regular_expression_node_t *)node;
        opening = literal->opening_loc;closing = literal->closing_loc;children = &literal->parts;break;
      }
      case PM_INTERPOLATED_MATCH_LAST_LINE_NODE: {
        const pm_interpolated_match_last_line_node_t *literal = (const pm_interpolated_match_last_line_node_t *)node;
        opening = literal->opening_loc;closing = literal->closing_loc;children = &literal->parts;break;
      }
      default: return;
    }
    if (!opening.start || !closing.start) return;
    sonicop_node_stack pending = {0}, parts = {0};
    bool eligible = true;
    if (children) { sonicop_push_list(&pending, children);eligible = sonicop_static_parts(&pending, &parts); }
    if (eligible) {
        size_t start = (size_t)(opening.start - source), end = (size_t)(closing.start + 1 - source);
        const uint8_t *flags = closing.start + 1;
        size_t flags_length = (size_t)(closing.end - flags);
        callback(data, start, end, NULL, 0, false, flags, flags_length, false);
        /* Translation::Parser は unescaped の String 自体の source encoding を渡す。
         * Ruby VM の literal flags による retag をここで加えない。 */
        if (plain && pm_string_length(plain)) {
            bool binary = source_binary;
            callback(data, start, end, pm_string_source(plain), pm_string_length(plain), binary, flags, flags_length, true);
        } else for (size_t i = 0; i < parts.size; i++) {
            // NULL は空の begin の正確な値（empty bytes、US-ASCII）を表す。
            if (!parts.nodes[i]) {
                callback(data, start, end, NULL, 0, 2, flags, flags_length, true);
                continue;
            }
            const pm_string_node_t *part = (const pm_string_node_t *)parts.nodes[i];
            bool binary = source_binary;
            callback(data, start, end, pm_string_source(&part->unescaped), pm_string_length(&part->unescaped), binary, flags, flags_length, true);
        }
    }
    free(pending.nodes);free(parts.nodes);
}
static void sonicop_collect_regexps(const pm_node_t *root, const uint8_t *source, bool binary,
                                  sonicop_prism_regexp_callback callback, void *data) {
    sonicop_node_stack pending = {0};sonicop_stack_push(&pending, root);
    while (pending.size && !pending.failed) {
        const pm_node_t *node = pending.nodes[--pending.size];
        sonicop_emit_regexp(node, source, binary, callback, data);
        size_t start = pending.size;
        pm_visit_child_nodes(node, sonicop_queue_child, &pending);
        for (size_t left = start, right = pending.size; left < right && left < --right; left++) {
            const pm_node_t *child = pending.nodes[left];pending.nodes[left] = pending.nodes[right];pending.nodes[right] = child;
        }
    }
    free(pending.nodes);
}

/* Ruby 側の C 構造体を Rust で再宣言せず、診断の値だけを同期的に渡す。 */
int sonicop_prism_compat_parse(const uint8_t *source, size_t length,
                             const char *version, size_t version_length,
                             bool binary, bool main_script,
                             sonicop_prism_diagnostic_callback callback, void *data,
                             sonicop_prism_regexp_callback regexp_callback, void *regexp_data) {
    pm_options_t options = {0};
    if (!pm_options_version_set(&options, version, version_length)) {
        pm_options_free(&options);
        return -1;
    }
    pm_options_partial_script_set(&options, !main_script);
    pm_options_main_script_set(&options, main_script);
    /* Translation::Parser の encoding:false は、復号済み UTF-8 の宣言を再解釈しない。 */
    /* スクリプト検査は実際の入出力バイト列を読み、宣言と BOM を Prism 自身に解釈させる。 */
    if (!binary && !main_script) {
        pm_options_encoding_set(&options, "UTF-8");
        pm_options_encoding_locked_set(&options, true);
    }
    pm_parser_t parser;
    pm_parser_init(&parser, source, length, &options);
    pm_node_t *node = pm_parse(&parser);
    for (pm_list_node_t *current = parser.error_list.head; current; current = current->next) {
        pm_diagnostic_t *error = (pm_diagnostic_t *)current;
        callback(data, (size_t)(error->location.start - source),
                 (size_t)(error->location.end - source),
                 pm_diagnostic_id_human(error->diag_id), error->message);
    }
    /* Prism の構造エラーがあれば、本家は builder へ進まない。 */
    if (!parser.error_list.head && regexp_callback) {
        sonicop_collect_regexps(node, source, binary, regexp_callback, regexp_data);
    }
    pm_node_destroy(&parser, node);
    pm_parser_free(&parser);
    pm_options_free(&options);
    return 0;
}

const char *sonicop_prism_compat_version(void) {
    return pm_version();
}


typedef void (*sonicop_prism_literal_callback)(void *, size_t, size_t, int);

typedef struct {
    const uint8_t *source;
    sonicop_prism_literal_callback callback;
    void *data;
} sonicop_prism_literal_context;

/* literal の区切りと character だけを渡し、本文や %w の WORDS_SEP は通常の木へ任せる。 */
static void sonicop_prism_literal_token(void *data, pm_parser_t *parser, pm_token_t *token) {
    sonicop_prism_literal_context *context = (sonicop_prism_literal_context *)data;
    int kind;
    switch (token->type) {
        case PM_TOKEN_STRING_BEGIN:
        case PM_TOKEN_REGEXP_BEGIN:
        case PM_TOKEN_PERCENT_LOWER_I:
        case PM_TOKEN_PERCENT_UPPER_I:
        case PM_TOKEN_PERCENT_LOWER_W:
        case PM_TOKEN_PERCENT_UPPER_W:
        case PM_TOKEN_PERCENT_LOWER_X:
            kind = 0;
            break;
        case PM_TOKEN_SYMBOL_BEGIN:
            /* 裸の :name は STRING_END を伴わない。引用符付きか %s の開始だけ積む。 */
            if (token->end - token->start <= 1) return;
            kind = 0;
            break;
        case PM_TOKEN_BACKTICK:
            /* 同じ token が def ` や obj.` のメソッド名にも使われる。 */
            if (parser->lex_modes.current->mode != PM_LEX_STRING) return;
            kind = 0;
            break;
        case PM_TOKEN_STRING_END:
        case PM_TOKEN_REGEXP_END:
        case PM_TOKEN_LABEL_END:
            kind = 1;
            break;
        case PM_TOKEN_CHARACTER_LITERAL:
            kind = 2;
            break;
        default:
            return;
    }
    context->callback(context->data, (size_t)(token->start - context->source),
                      (size_t)(token->end - context->source), kind);
}

/* 制御文字がある希少経路だけ呼ぶ。診断と同じ parser と反復的な解放を使う。 */
void sonicop_prism_compat_literal_tokens(const uint8_t *source, size_t length,
                                       sonicop_prism_literal_callback callback, void *data) {
    pm_options_t options = {0};
    pm_options_encoding_set(&options, "UTF-8");
    pm_options_encoding_locked_set(&options, true);
    pm_options_partial_script_set(&options, true);
    pm_parser_t parser;
    pm_parser_init(&parser, source, length, &options);
    sonicop_prism_literal_context context = {source, callback, data};
    pm_lex_callback_t lex = {&context, sonicop_prism_literal_token};
    parser.lex_callback = &lex;
    pm_node_t *node = pm_parse(&parser);
    pm_node_destroy(&parser, node);
    pm_parser_free(&parser);
    pm_options_free(&options);
}
