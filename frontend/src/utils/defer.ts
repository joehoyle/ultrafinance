import { defer as defer1 } from 'react-router-dom';

export interface AwaitProps1<T> {
	children: (data: T) => React.ReactNode;
	errorElement?: React.ReactNode;
	resolve: Promise<T>;
}

export declare function Await<T>(props: AwaitProps1<T>): JSX.Element;
export function defer<T extends object>(obj: T): IDeferredData<T> {
	return defer1(obj as Record<string, string>) as unknown as IDeferredData<T>;
}

// Define an interface with mapped types
export type IDeferredData<T> = {
	[K in keyof T]: T[K];
}
