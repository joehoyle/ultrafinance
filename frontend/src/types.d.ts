import React from 'react';

export {};

declare global {
	function foo(): any;
}

declare module 'react-router-dom' {
	export interface AwaitProps1<T> {
		children: (data: T) => React.ReactNode;
		errorElement?: React.ReactNode;
		resolve: Promise<T>;
	}

	export declare function Await<T>(props: AwaitProps1<T>): JSX.Element;
	export function defer<T extends object>(obj: T): { [K in keyof T]: Promise<Awaited<T[K]>> };
}
