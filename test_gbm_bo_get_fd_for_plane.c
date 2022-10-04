#include <gbm.h>

void test() {
    gbm_bo_get_fd_for_plane(NULL, 0);
}