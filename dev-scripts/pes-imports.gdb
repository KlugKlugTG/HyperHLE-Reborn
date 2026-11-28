set confirm off
set pagination off
set breakpoint pending off
set language c
handle SIGSEGV nostop noprint pass
python
import subprocess
syms = [line.split()[-1] for line in subprocess.check_output(["nm", gdb.current_progspace().filename], text=True).splitlines() if line.split()]
stub = next(s for s in syms if s.endswith("unimplemented_function_stub"))
pointer = next(s for s in syms if "LAST_HOST_CALL_SYMBOL_PTR" in s)
length = next(s for s in syms if "LAST_HOST_CALL_SYMBOL_LEN" in s)
gdb.execute("break *'%s'" % stub)
end
commands
silent
python
p = int(gdb.parse_and_eval("*(unsigned long *)&'%s'" % pointer))
n = int(gdb.parse_and_eval("*(unsigned long *)&'%s'" % length))
print('UNIMPLEMENTED CALLED', bytes(gdb.selected_inferior().read_memory(p, n)).decode())
end
continue
end
info breakpoints
run
