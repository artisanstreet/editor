// Opt-in diagnosis after idle sampling. Each call briefly suspends one thread
// belonging to the runner-owned child, reads x64 registers and two pointed-to
// words, then resumes it. It never changes registers or process memory.
using System;
using System.ComponentModel;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;

public static class ArtisanThreadContext {
    public sealed class Sample {
        public string instruction_address, stack_address;
        public uint prior_suspend_count;
        public double suspended_microseconds;
        public Dictionary<string, string> integer_registers;
        public string return_address;
        public long? value_at_r8;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenThread(uint access, bool inherit, int threadId);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint GetProcessIdOfThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint SuspendThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool GetThreadContext(IntPtr thread, IntPtr context);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, int processId);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool ReadProcessMemory(IntPtr process, IntPtr address,
        out long value, UIntPtr bytes, out UIntPtr read);
    [DllImport("kernel32.dll")]
    private static extern bool CloseHandle(IntPtr handle);

    public static Sample Read(int expectedProcessId, int threadId) {
        if (IntPtr.Size != 8) throw new NotSupportedException("x64 PowerShell required.");
        IntPtr thread = OpenThread(0x2 | 0x8 | 0x800, false, threadId);
        if (thread == IntPtr.Zero) throw new Win32Exception();
        IntPtr storage = IntPtr.Zero;
        IntPtr process = IntPtr.Zero;
        try {
            if (GetProcessIdOfThread(thread) != (uint)expectedProcessId)
                throw new InvalidOperationException("Thread does not belong to the expected child.");
            process = OpenProcess(0x10, false, expectedProcessId);
            if (process == IntPtr.Zero) throw new Win32Exception();
            // AMD64 CONTEXT is 1232 bytes, aligned to 16. These offsets are
            // from the Windows SDK CONTEXT layout: ContextFlags=48, Rsp=152,
            // Rip=248. Request CONTROL plus INTEGER register groups.
            storage = Marshal.AllocHGlobal(1232 + 15);
            IntPtr context = new IntPtr((storage.ToInt64() + 15) & ~15L);
            Marshal.Copy(new byte[1232], 0, context, 1232);
            Marshal.WriteInt32(context, 48, 0x00100003);
            var sample = new Sample();
            long[] registers = new long[16];
            long rip = 0, rsp = 0;
            long returnAddress = 0, r8Value = 0;
            bool readReturnAddress = false, readR8 = false;
            UIntPtr read;
            long started = Stopwatch.GetTimestamp();
            uint prior = SuspendThread(thread);
            if (prior == UInt32.MaxValue) throw new Win32Exception();
            try {
                if (!GetThreadContext(thread, context)) throw new Win32Exception();
                rip = Marshal.ReadInt64(context, 248);
                rsp = Marshal.ReadInt64(context, 152);
                Marshal.Copy(IntPtr.Add(context, 120), registers, 0, registers.Length);
                // For the sampled NtWaitForSingleObject syscall boundary,
                // [RSP] is its immediate caller and R8 points to the timeout.
                // Read only these two words; interpret them after module lookup.
                readReturnAddress = ReadProcessMemory(process, new IntPtr(rsp),
                    out returnAddress, new UIntPtr(8), out read) && read.ToUInt64() == 8;
                if (registers[8] >= 0x10000 && registers[8] < 0x0000800000000000L) {
                    readR8 = ReadProcessMemory(process, new IntPtr(registers[8]),
                        out r8Value, new UIntPtr(8), out read) && read.ToUInt64() == 8;
                }
            } finally {
                // Undo exactly our one suspension, preserving any prior count.
                if (ResumeThread(thread) == UInt32.MaxValue) throw new Win32Exception();
            }
            sample.suspended_microseconds = (Stopwatch.GetTimestamp() - started)
                * 1000000.0 / Stopwatch.Frequency;
            sample.prior_suspend_count = prior;
            sample.instruction_address = "0x" + rip.ToString("x");
            sample.stack_address = "0x" + rsp.ToString("x");
            string[] names = { "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
                "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15" };
            sample.integer_registers = new Dictionary<string, string>();
            for (int index = 0; index < names.Length; index++)
                sample.integer_registers[names[index]] = "0x" + registers[index].ToString("x");
            if (readReturnAddress) sample.return_address = "0x" + returnAddress.ToString("x");
            if (readR8) sample.value_at_r8 = r8Value;
            return sample;
        } finally {
            if (storage != IntPtr.Zero) Marshal.FreeHGlobal(storage);
            if (process != IntPtr.Zero) CloseHandle(process);
            CloseHandle(thread);
        }
    }
}
