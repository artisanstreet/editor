import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

export const cn = (...inputs: Array<ClassValue>) => twMerge(clsx(inputs));

export type WithElementRef<T> = T & { ref?: HTMLElement | null };

export type WithoutChildren<T> = Omit<T, "children">;

export type WithoutChild<T> = Omit<T, "child">;

export type WithoutChildrenOrChild<T> = WithoutChildren<WithoutChild<T>>;
