/* Ruby の GC 領域を使わず、整数 wraparound を意図した hash だけを同じ設定にする。 */
#ifndef SONICOP_STANDALONE_SANITIZERS_H
#define SONICOP_STANDALONE_SANITIZERS_H
#include <stdbool.h>
#ifdef __clang__
#define NO_SANITIZE(kind, declaration) __attribute__((no_sanitize(kind))) declaration
#else
#define NO_SANITIZE(kind, declaration) declaration
#endif
#endif
