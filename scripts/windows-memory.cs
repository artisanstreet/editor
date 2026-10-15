// Read-only resident-page attribution. Does not read page contents, fault pages
// into the target, trim its working set, or suspend any target thread.
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class ArtisanMemoryInspector {
    [StructLayout(LayoutKind.Sequential)]
    private struct MemoryInfo {
        public IntPtr BaseAddress, AllocationBase;
        public uint AllocationProtect;
        public UIntPtr RegionSize;
        public uint State, Protect, Type;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct WorkingSetPage {
        public IntPtr Address;
        public UIntPtr Flags;
    }
    public sealed class Region {
        public string address, allocation, kind, state, mapped_file;
        public ulong bytes, resident_bytes, private_resident_bytes, shared_resident_bytes;
        public ulong working_set_bytes, working_set_private_bytes, working_set_shared_bytes;
        public uint protection;
        public bool guarded;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenProcess(uint access, bool inherit, int id);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern UIntPtr VirtualQueryEx(IntPtr process, IntPtr address,
        out MemoryInfo info, UIntPtr length);
    [DllImport("psapi.dll", SetLastError = true)]
    private static extern bool QueryWorkingSetEx(IntPtr process,
        [In, Out] WorkingSetPage[] pages, uint bytes);
    [DllImport("psapi.dll", SetLastError = true)]
    private static extern bool QueryWorkingSet(IntPtr process, IntPtr buffer, uint bytes);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetMappedFileName(IntPtr process, IntPtr address,
        StringBuilder path, uint length);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr OpenThread(uint access, bool inherit, int id);
    [DllImport("kernel32.dll")]
    private static extern int GetThreadDescription(IntPtr thread, out IntPtr text);
    [DllImport("kernel32.dll")]
    private static extern IntPtr LocalFree(IntPtr pointer);
    [DllImport("ntdll.dll")]
    private static extern int NtQueryInformationThread(IntPtr thread, int informationClass,
        out IntPtr startAddress, uint length, out uint returnedLength);

    public static string ThreadStartAddress(int threadId) {
        IntPtr thread = OpenThread(0x40, false, threadId);
        if (thread == IntPtr.Zero) return null;
        try {
            IntPtr address;
            uint length;
            // ThreadQuerySetWin32StartAddress; this asks for metadata only.
            if (NtQueryInformationThread(thread, 9, out address, (uint)IntPtr.Size, out length) < 0)
                return null;
            return "0x" + address.ToInt64().ToString("x");
        } finally { CloseHandle(thread); }
    }

    public static string ThreadDescription(int threadId) {
        IntPtr thread = OpenThread(0x800, false, threadId);
        if (thread == IntPtr.Zero) return null;
        try {
            IntPtr text;
            if (GetThreadDescription(thread, out text) < 0) return null;
            try { return Marshal.PtrToStringUni(text); }
            finally { LocalFree(text); }
        } finally { CloseHandle(thread); }
    }

    public static Region[] Inspect(int processId) {
        if (IntPtr.Size != 8) throw new NotSupportedException("Use 64-bit PowerShell.");
        // QUERY_INFORMATION and VM_READ allow metadata queries. No write access.
        IntPtr process = OpenProcess(0x400 | 0x10, false, processId);
        if (process == IntPtr.Zero) throw new Win32Exception();
        try {
            var regions = new List<Region>();
            var paths = new Dictionary<IntPtr, string>();
            var pages = new WorkingSetPage[4096];
            uint pageInfoBytes = (uint)Marshal.SizeOf(typeof(WorkingSetPage));
            ulong pageBytes = (ulong)Environment.SystemPageSize;
            ulong address = 0;
            while (address < 0x0000800000000000UL) {
                MemoryInfo info;
                UIntPtr queried = VirtualQueryEx(process, new IntPtr((long)address),
                    out info, new UIntPtr((uint)Marshal.SizeOf(typeof(MemoryInfo))));
                if (queried == UIntPtr.Zero) {
                    int error = Marshal.GetLastWin32Error();
                    if (error == 87) break; // Past the target's user address range.
                    throw new Win32Exception(error);
                }
                ulong size = info.RegionSize.ToUInt64();
                if (size == 0 || address + size <= address)
                    throw new InvalidOperationException("Invalid virtual-memory region size.");
                address = (ulong)info.BaseAddress.ToInt64() + size;
                if (info.State == 0x10000) continue; // MEM_FREE
                var region = new Region {
                    address = "0x" + info.BaseAddress.ToInt64().ToString("x"),
                    allocation = "0x" + info.AllocationBase.ToInt64().ToString("x"),
                    kind = info.Type == 0x1000000 ? "image" :
                           info.Type == 0x40000 ? "mapped" :
                           info.Type == 0x20000 ? "private" : "unknown",
                    state = info.State == 0x1000 ? "committed" : "reserved",
                    bytes = size,
                    protection = info.Protect,
                    guarded = (info.Protect & 0x100) != 0,
                    mapped_file = ""
                };
                if (info.State == 0x1000) {
                    if (info.Type != 0x20000) {
                        string path;
                        if (!paths.TryGetValue(info.AllocationBase, out path)) {
                            var buffer = new StringBuilder(4096);
                            path = GetMappedFileName(process, info.BaseAddress, buffer,
                                (uint)buffer.Capacity) == 0 ? "" : buffer.ToString();
                            paths[info.AllocationBase] = path;
                        }
                        region.mapped_file = path;
                    }
                    ulong offset = 0;
                    while (offset < size) {
                        int count = (int)Math.Min((ulong)pages.Length, (size - offset) / pageBytes);
                        if (count == 0) throw new InvalidOperationException("Unaligned region.");
                        for (int index = 0; index < count; index++) {
                            pages[index].Address = new IntPtr(info.BaseAddress.ToInt64()
                                + (long)(offset + (ulong)index * pageBytes));
                            pages[index].Flags = UIntPtr.Zero;
                        }
                        if (!QueryWorkingSetEx(process, pages, (uint)count * pageInfoBytes))
                            throw new Win32Exception();
                        for (int index = 0; index < count; index++) {
                            ulong flags = pages[index].Flags.ToUInt64();
                            if ((flags & 1) == 0) continue;
                            region.resident_bytes += pageBytes;
                            if ((flags & (1UL << 15)) == 0)
                                region.private_resident_bytes += pageBytes;
                            if (((flags >> 1) & 7) > 1)
                                region.shared_resident_bytes += pageBytes;
                        }
                        offset += (ulong)count * pageBytes;
                    }
                }
                regions.Add(region);
            }
            AttributeWorkingSet(process, regions);
            return regions.ToArray();
        } finally {
            CloseHandle(process);
        }
    }

    // QueryWorkingSetEx can identify resident pages outside this process's
    // working set. Enumerate actual membership separately to reconcile RSS.
    private static void AttributeWorkingSet(IntPtr process, List<Region> regions) {
        int capacity = 1024 * 1024;
        IntPtr buffer = IntPtr.Zero;
        try {
            while (true) {
                if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
                buffer = Marshal.AllocHGlobal(capacity);
                if (QueryWorkingSet(process, buffer, (uint)capacity)) break;
                int error = Marshal.GetLastWin32Error();
                if (error != 24 || capacity >= 128 * 1024 * 1024)
                    throw new Win32Exception(error);
                capacity *= 2;
            }
            long count = Marshal.ReadInt64(buffer);
            if (count < 0 || count > capacity / 8 - 1)
                throw new InvalidOperationException("Invalid working-set page count.");
            var bases = new ulong[regions.Count];
            for (int index = 0; index < regions.Count; index++)
                bases[index] = Convert.ToUInt64(regions[index].address.Substring(2), 16);
            for (int index = 0; index < (int)count; index++) {
                ulong flags = unchecked((ulong)Marshal.ReadInt64(buffer, (index + 1) * 8));
                ulong page = flags & 0xfffffffffffff000UL;
                int regionIndex = Array.BinarySearch(bases, page);
                if (regionIndex < 0) regionIndex = ~regionIndex - 1;
                if (regionIndex < 0 || page >= bases[regionIndex] + regions[regionIndex].bytes)
                    continue; // Mapping changed between the two metadata snapshots.
                Region region = regions[regionIndex];
                region.working_set_bytes += (ulong)Environment.SystemPageSize;
                if ((flags & (1UL << 8)) == 0)
                    region.working_set_private_bytes += (ulong)Environment.SystemPageSize;
                if (((flags >> 5) & 7) > 1)
                    region.working_set_shared_bytes += (ulong)Environment.SystemPageSize;
            }
        } finally {
            if (buffer != IntPtr.Zero) Marshal.FreeHGlobal(buffer);
        }
    }
}
