(module
  (type (;0;) (func))
  (type (;1;) (func (param i32 i32) (result i32)))
  (import "env" "foo" (func (;0;) (type 0)))
  (import "env" "boo" (func (;1;) (type 0)))
  (func (;2;) (type 1) (param i32 i32) (result i32)
    call 0
    call 1
    local.get 0
    local.get 1
    i32.add)
  (func (;3;) (type 1) (param i32 i32) (result i32)
    local.get 0
    local.get 1
    global.get 0
    i32.const 2
    i32.add
    global.set 0
    global.get 0
    i32.const 1024
    i32.gt_u
    if  ;; label = @1
      unreachable
    end
    call 2
    global.get 0
    i32.const 2
    i32.sub
    global.set 0)
  (global (;0;) (mut i32) (i32.const 0))
  (export "i32.add" (func 3)))
