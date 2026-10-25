#include "quickjs.h"

#include "pocket_runtime.h"

void pocket_bridge_free_value(JSContext *ctx, JSValue value) {
  JS_FreeValue(ctx, value);
}
