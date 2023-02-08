#include <gbm.h>

void test() {
    gbm_bo_create_with_modifiers2(NULL, 0, 0, 0, NULL, 0, 0);
}