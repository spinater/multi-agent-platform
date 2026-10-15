"use client";

/** The interface language (EN | TH; design: LangSwitch.dc.html). English by default; the choice is kept in
 * `localStorage['map.lang']` and broadcast with a `map-lang` window event, so every open screen re-renders. Copy that
 * is translated lives in the catalogue below (`en` is the source; a key missing in `th` falls back to English).
 * Config files and what agents write stay in English unless Configure's config language says otherwise. */

import { useSyncExternalStore } from "react";

export type Lang = "en" | "th";

const KEY = "map.lang";
const EVENT = "map-lang";

function read(): Lang {
  try {
    return localStorage.getItem(KEY) === "th" ? "th" : "en";
  } catch {
    return "en";
  }
}

export function setLang(l: Lang) {
  try {
    localStorage.setItem(KEY, l);
  } catch {
    // Storage blocked: the choice lasts for this page.
  }
  window.dispatchEvent(new Event(EVENT));
}

function subscribe(cb: () => void) {
  window.addEventListener(EVENT, cb);
  window.addEventListener("storage", cb);
  return () => {
    window.removeEventListener(EVENT, cb);
    window.removeEventListener("storage", cb);
  };
}

export function useLang(): Lang {
  return useSyncExternalStore(subscribe, read, () => "en");
}

const en = {
  "lang.title": "Interface language",
  "lang.toggle": "Switch the interface to Thai",
  "sw.title": "Switch project",
  "sw.default": "default workspace",
  "sw.all": "All projects",
  "sw.allSub": "merged view, read-only",
  "sw.new": "+ New project",
  "top.settings": "Settings: git and SSH access, agent CLIs, server",
  "top.theme": "Toggle theme",
  "cc.view": "VIEW",
  "cc.graph": "GRAPH",
  "cc.orbit": "3D ORBIT",
  "cc.activity": "ACTIVITY",
  "cc.orchestrator": "Orchestrator",
  "cc.running": "Running",
  "cc.paused": "Paused",
  "cc.allProjects": "All projects",
  "cc.needsYou": "Needs you",
  "cc.live": "Live activity",
  "cc.sessions": "Sessions",
  "cc.events": "Event log",
  "cc.queue": "Task queue",
  "cc.allTasks": "All tasks",
  "cc.lanes": "Lanes",
  "cc.newTask": "+ New task",
  "cc.new": "+ New",
  "cc.addLane": "+ Add lane",
  "tab.agents": "Agents",
  "tab.workflows": "Workflows",
  "tab.rules": "Rules",
  "tab.skills": "Skills",
  "tab.perms": "Permissions",
  "tab.hooks": "Hooks",
  "tab.verify": "Verify & audit",
  "tab.git": "Git & SSH",
  "tab.branches": "Repo & branches",
  "tab.parallel": "Parallel",
  "tab.mcp": "MCP servers",
  "tab.env": "Env & secrets",
  "tab.providers": "Providers",
  "set.git": "Git & SSH",
  "set.agents": "Agent CLIs",
  "set.gates": "Reflex gates",
  "set.telegram": "Telegram",
  "set.server": "Server",
  "nav.projects": "Projects",
  "nav.command": "Command centre",
  "nav.configure": "Configure",
  "nav.knowledge": "Knowledge",
  "nav.decisions": "Decisions",
  "nav.reflex": "Reflex",
  "nav.usage": "Usage",
  "nav.lanes": "Lanes",
  "nav.skills": "Skills",
  "nav.agents": "Agents",
  "nav.settings": "Settings",
  "cfg.title": "Configure",
  "cfg.intro": "Agents, workflows and everything they run with. Machine settings apply to every project; a project can override any of them, and each item shows where its value comes from.",
  "cfg.kProject": "Project · {p} · machine values shown where not overridden",
  "cfg.kMachine": "Machine · every project on this computer",
  "cfg.project": "Project",
  "cfg.machine": "Machine",
  "cfg.allProjects": "all projects",
  "cfg.pickFirst": "Pick a project in the switcher first",
  "cfg.cfgLang": "config language",
  "cfg.english": "English · recommended",
  "cfg.thaiWarn":
    "Config is set to Thai. Rules, prompts and generated config will be written in Thai. Not recommended: Thai text usually costs more tokens than the same text in English, on every session that loads it, and agents follow English instructions more reliably.",
  "cfg.backToEn": "Use English",
  "cfg.lgMachine": "every project",
  "cfg.lgProject": "this project only",
  "cfg.lgOverride": "replaces machine here",
  "cfg.group.Agents": "Agents",
  "cfg.group.Context": "Context",
  "cfg.group.Safety": "Safety",
  "cfg.group.Git": "Git",
  "cfg.group.Connections": "Connections",
  "cfg.savesTo": "saves to",
  "cfg.unsaved": "{n} unsaved change{s}",
  "cfg.discard": "Discard",
  "cfg.save": "Save",
  "cfg.saving": "Saving…",
  "cfg.saved": "Saved",
  "cfg.discarded": "Discarded changes",
  "cfg.switchConfirm": "{n} unsaved changes will be discarded. Switch anyway?",
  "ai.title": "Ask an agent to do it",
  "ai.sub": "Describe what you want in plain words. You review the changes before anything is applied.",
  "ai.outEn": "writes config in English",
  "ai.outEnTh": "your prompt is in Thai; the config is still written in English (change it in config language)",
  "ai.outTh": "writes config in Thai · uses more tokens",
  "ai.generate": "✦ Generate",
  "ai.apply": "Apply {n} change{s}",
} as const;

export type MsgKey = keyof typeof en;

/** Thai: the nav and Configure's chrome (header, scope, tab groups and descriptions, AI panel, save bar, warnings). */
const th: Partial<Record<MsgKey, string>> = {
  "lang.title": "ภาษาของหน้าจอ",
  "lang.toggle": "สลับหน้าจอเป็นภาษาอังกฤษ",
  "sw.title": "สลับ project",
  "sw.default": "workspace หลัก",
  "sw.all": "ทุก project",
  "sw.allSub": "มุมมองรวม อ่านอย่างเดียว",
  "sw.new": "+ สร้าง project",
  "top.settings": "ตั้งค่าระบบ: git และ SSH, CLI ของ agent, เซิร์ฟเวอร์",
  "top.theme": "สลับธีม",
  "cc.view": "มุมมอง",
  "cc.graph": "กราฟ",
  "cc.orbit": "วงโคจร 3D",
  "cc.activity": "กิจกรรม",
  "cc.orchestrator": "ตัวจัดงาน",
  "cc.running": "กำลังรัน",
  "cc.paused": "หยุดอยู่",
  "cc.allProjects": "ทุก project",
  "cc.needsYou": "รอคุณ",
  "cc.live": "ความเคลื่อนไหว",
  "cc.sessions": "เซสชัน",
  "cc.events": "บันทึกเหตุการณ์",
  "cc.queue": "คิวงาน",
  "cc.allTasks": "งานทั้งหมด",
  "cc.lanes": "เลน",
  "cc.newTask": "+ งานใหม่",
  "cc.new": "+ ใหม่",
  "cc.addLane": "+ เพิ่มเลน",
  "tab.agents": "Agent",
  "tab.workflows": "เวิร์กโฟลว์",
  "tab.rules": "กฎ",
  "tab.skills": "สกิล",
  "tab.perms": "สิทธิ์",
  "tab.hooks": "ฮุก",
  "tab.verify": "ตรวจและออดิต",
  "tab.git": "Git และ SSH",
  "tab.branches": "รีโปและ branch",
  "tab.parallel": "งานคู่ขนาน",
  "tab.mcp": "MCP server",
  "tab.env": "ตัวแปรและความลับ",
  "tab.providers": "ผู้ให้บริการ",
  "set.git": "Git และ SSH",
  "set.agents": "CLI ของ agent",
  "set.gates": "Reflex gates",
  "set.telegram": "Telegram",
  "set.server": "เซิร์ฟเวอร์",
  "nav.projects": "โปรเจกต์",
  "nav.command": "ศูนย์ควบคุม",
  "nav.configure": "ตั้งค่า",
  "nav.knowledge": "ความรู้",
  "nav.decisions": "การตัดสินใจ",
  "nav.reflex": "Reflex",
  "nav.usage": "การใช้งาน",
  "nav.lanes": "เลน",
  "nav.skills": "สกิล",
  "nav.agents": "เอเจนต์",
  "nav.settings": "ตั้งค่าระบบ",
  "cfg.title": "ตั้งค่า",
  "cfg.intro": "Agent, workflow และทุกอย่างที่ agent ใช้ทำงาน ค่าของเครื่องใช้กับทุก project และแต่ละ project ทับค่าเหล่านั้นได้ ทุกรายการบอกว่าค่ามาจากไหน",
  "cfg.kProject": "Project · {p} · แสดงค่าของเครื่องในส่วนที่ไม่ได้ทับ",
  "cfg.kMachine": "เครื่องนี้ · ทุก project ในคอมพิวเตอร์เครื่องนี้",
  "cfg.project": "Project",
  "cfg.machine": "เครื่องนี้",
  "cfg.allProjects": "ทุก project",
  "cfg.pickFirst": "เลือก project ที่ตัวสลับก่อน",
  "cfg.cfgLang": "ภาษาของ config",
  "cfg.english": "English · แนะนำ",
  "cfg.thaiWarn":
    "ตั้ง config เป็นภาษาไทยอยู่ rule, prompt และ config ที่สร้างจะเป็นภาษาไทย ไม่แนะนำ: ข้อความภาษาไทยมักใช้ token มากกว่าข้อความเดียวกันในภาษาอังกฤษ และเสียทุก session ที่โหลด อีกทั้ง agent ทำตามคำสั่งภาษาอังกฤษได้แม่นกว่า",
  "cfg.backToEn": "ใช้ภาษาอังกฤษ",
  "cfg.lgMachine": "ทุก project",
  "cfg.lgProject": "เฉพาะ project นี้",
  "cfg.lgOverride": "ทับค่าของเครื่อง",
  "cfg.group.Agents": "Agent",
  "cfg.group.Context": "บริบท",
  "cfg.group.Safety": "ความปลอดภัย",
  "cfg.group.Git": "Git",
  "cfg.group.Connections": "การเชื่อมต่อ",
  "cfg.savesTo": "บันทึกที่",
  "cfg.unsaved": "ยังไม่บันทึก {n} รายการ",
  "cfg.discard": "ยกเลิก",
  "cfg.save": "บันทึก",
  "cfg.saving": "กำลังบันทึก…",
  "cfg.saved": "บันทึกแล้ว",
  "cfg.discarded": "ยกเลิกการแก้ไขแล้ว",
  "cfg.switchConfirm": "การแก้ไขที่ยังไม่บันทึก {n} รายการจะหายไป สลับต่อไหม",
  "ai.title": "ให้ agent จัดการให้",
  "ai.sub": "พิมพ์สิ่งที่ต้องการเป็นภาษาธรรมดา ระบบจะให้ตรวจการเปลี่ยนแปลงก่อนใช้ทุกครั้ง",
  "ai.outEn": "เขียน config เป็นภาษาอังกฤษ",
  "ai.outEnTh": "prompt เป็นภาษาไทยได้ แต่ config จะยังเขียนเป็นภาษาอังกฤษ (เปลี่ยนได้ที่ ภาษาของ config)",
  "ai.outTh": "เขียน config เป็นภาษาไทย · ใช้ token มากขึ้น",
  "ai.generate": "✦ สร้าง",
  "ai.apply": "ใช้ {n} รายการ",
};

/** Configure's tab titles in Thai, by English title (TabHead). */
export const TAB_TITLE_TH: Record<string, string> = {
  Agents: "Agent",
  Workflows: "เวิร์กโฟลว์",
  Rules: "กฎ",
  Skills: "สกิล",
  Permissions: "สิทธิ์",
  Hooks: "ฮุก",
  "Verify & audit": "ตรวจและออดิต",
  "Git & SSH": "Git และ SSH",
  "Repo & branches": "รีโปและ branch",
  Parallel: "งานคู่ขนาน",
  "MCP servers": "MCP server",
  "Env & secrets": "ตัวแปรและความลับ",
  Providers: "ผู้ให้บริการ",
};

/** Configure's tab descriptions in Thai, by tab title (the English ones live with each tab). */
export const TAB_DESC_TH: Record<string, string> = {
  Agents: "provider และ model พร้อม role, skill, rule, tool และ MCP server ที่ได้ใช้ และจำนวนที่รันพร้อมกันได้",
  Workflows: "agent ตัวไหนทำงานกับ task ลำดับอย่างไร และมี gate กับ hook อะไรคั่น ให้ agent แนะนำการปรับจากประวัติ run ล่าสุดได้",
  Rules: "ไฟล์ context ที่ agent อ่านตอนเริ่มทุก session โดย CLAUDE.md และ GEMINI.md import AGENTS.md ทุก agent จึงทำตามกฎชุดเดียวกัน",
  Skills: "เปิดหรือปิด skill สำหรับ project นี้ ส่วน agent แต่ละตัวโหลด skill ไหนเลือกได้ที่แท็บ Agents",
  Permissions: "CLI แต่ละตัวทำอะไรได้โดยไม่ต้องถาม ถ้าเป็น ask session จะหยุดรอจนมีคนตอบในศูนย์ควบคุม",
  Hooks: "คำสั่งที่ MAP รันรอบๆ task ถ้า hook แบบ blocking fail task จะหยุดรอคนตัดสินใจ",
  "Verify & audit": "วิธีพิสูจน์ว่า task เสร็จจริง เมื่อ workflow ไม่ได้กำหนดไว้",
  "MCP servers": "server เครื่องมือที่ agent เรียกใช้ได้ CLI ที่มี server นั้นใช้ได้ และ agent ได้ server ของ CLI ที่มันเขียนไฟล์ให้",
  "Env & secrets": "ตัวแปรที่ส่งให้ทุก session และ hook ใน project นี้",
  "Git & SSH": "agent clone, pull และ push ผ่าน SSH ชุดเดียวกันทุก project ใช้ key ของ MAP เองหรือ ~/.ssh ของเครื่อง",
  "Repo & branches": "สถานะ git ของโฟลเดอร์ project และ task รันในโฟลเดอร์เดียวกันหรือแยก branch/worktree ต่อ task",
  Parallel: "project นี้รัน task พร้อมกันได้กี่ตัว และใครตัดสินว่า task ไหนรันคู่กันได้",
  Providers: "CLI ของ agent บนเครื่องนี้ สถานะการ login และลำดับ",
};

/** `t(key, vars)` in the current language; `{s}` becomes "s" when `n` is not 1 (English plurals). */
export function useT() {
  const lang = useLang();
  return (key: MsgKey, vars: Record<string, string | number> = {}) => {
    let s: string = (lang === "th" ? th[key] : undefined) ?? en[key];
    const all = { s: vars.n === undefined || Number(vars.n) === 1 ? "" : "s", ...vars };
    for (const [k, v] of Object.entries(all)) s = s.replaceAll(`{${k}}`, String(v));
    return s;
  };
}
