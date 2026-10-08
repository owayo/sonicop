#include "rename.h"
#include "prism.h"
#include <stddef.h>

typedef void (*sonicop_prism_diagnostic_callback)(void *, size_t, size_t, const char *, const char *);

/* Ruby 側の C 構造体を Rust で再宣言せず、診断の値だけを同期的に渡す。 */
int sonicop_prism_compat_parse(const uint8_t *source, size_t length,
                             const char *version, size_t version_length,
                             bool binary,
                             sonicop_prism_diagnostic_callback callback, void *data) {
    pm_options_t options = {0};
    if (!pm_options_version_set(&options, version, version_length)) {
        pm_options_free(&options);
        return -1;
    }
    pm_options_partial_script_set(&options, true);
    /* Translation::Parser の encoding:false は、復号済み UTF-8 の宣言を再解釈しない。 */
    if (!binary) {
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
