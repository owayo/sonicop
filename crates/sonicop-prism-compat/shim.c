#include "rename.h"
#include "prism.h"
#include <stddef.h>

typedef void (*sonicop_prism_diagnostic_callback)(void *, size_t, size_t, const char *, const char *);

/* Ruby 側の C 構造体を Rust で再宣言せず、診断の値だけを同期的に渡す。 */
int sonicop_prism_compat_parse(const uint8_t *source, size_t length,
                             const char *version, size_t version_length,
                             bool binary, bool main_script,
                             sonicop_prism_diagnostic_callback callback, void *data) {
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
