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
	export type DeferFunction<T> = (data: T, init?: number | ResponseInit) => IDeferredData<T>;
	export declare const defer: DeferFunction;

	// Define an interface with mapped types
	export type IDeferredData<T> = {
		[K in keyof T]: T[K];
	}

}

declare module 'react-router-dom' {

}
