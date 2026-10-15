#include <stdlib.h>

typedef struct {
    pm_node_t **nodes;
    size_t size;
    size_t capacity;
} sonicop_pending_nodes_t;

/* 最外の呼び出しが所有する配列への参照だけを保持し、ワーカー間で混ぜない。 */
#ifdef _MSC_VER
static __declspec(thread) sonicop_pending_nodes_t *sonicop_pending_nodes;
#else
static _Thread_local sonicop_pending_nodes_t *sonicop_pending_nodes;
#endif

static void
sonicop_queue_node(sonicop_pending_nodes_t *pending, pm_node_t *node) {
    if (pending->size == pending->capacity) {
        if (pending->capacity > SIZE_MAX / 2 / sizeof(pm_node_t *)) abort();
        size_t capacity = pending->capacity == 0 ? 64 : pending->capacity * 2;
        pm_node_t **nodes = realloc(pending->nodes, capacity * sizeof(pm_node_t *));
        /* 確保失敗時も子を捨てたり再帰に戻したりせず、Rust の確保失敗と同じく停止する。 */
        if (nodes == NULL) abort();
        pending->nodes = nodes;
        pending->capacity = capacity;
    }
    pending->nodes[pending->size++] = node;
}

PRISM_EXPORTED_FUNCTION void
pm_node_destroy(pm_parser_t *parser, pm_node_t *node) {
    if (sonicop_pending_nodes != NULL) {
        sonicop_queue_node(sonicop_pending_nodes, node);
        return;
    }
    sonicop_pending_nodes_t pending = {0};
    sonicop_pending_nodes = &pending;
    sonicop_node_destroy_one(parser, node);
    while (pending.size > 0) {
        sonicop_node_destroy_one(parser, pending.nodes[--pending.size]);
    }
    sonicop_pending_nodes = NULL;
    free(pending.nodes);
}
