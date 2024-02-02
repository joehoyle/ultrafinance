import { Suspense } from "react";
import { Await, defer, Link, ScrollRestoration, useLoaderData } from "react-router-dom";
import { User } from "../../../bindings/User";
import { getMe } from "../api";
import Footer from "../components/Footer";

export function loader() {
	return defer({
		user: getMe(),
	})
}

export default function Homepage() {
	const { user } = useLoaderData() as ReturnType<typeof loader>;
	return <>
		<ScrollRestoration />
		<div className="p-8 min-h-screen flex flex-col from-purple-700 to-purple-900 bg-gradient-to-t">
			<div className="text-white flex justify-between">
				<svg width="40px" height="40px" viewBox="0 0 300 300">
					<defs>
						<clipPath id="Group">
							<polygon points="156.040268 4.54747351e-13 206.375839 4.54747351e-13 206.375839 195.06897 156.040268 300"></polygon>
							<polygon transform="translate(234.060403, 25.000000) rotate(-270.000000) translate(-234.060403, -25.000000) " points="209.060403 -40.9395973 259.060403 -16.9210754 259.060403 90.9395973 209.060403 90.9395973"></polygon>
							<polygon transform="translate(213.963533, 115.000000) rotate(-270.000000) translate(-213.963533, -115.000000) " points="188.963533 72.2521235 238.963533 96.2706454 238.963533 157.747876 188.963533 157.747876"></polygon>
							<polygon points="93.6241611 4.54747351e-13 143.959732 4.54747351e-13 143.959732 300 97.147651 173.03125"></polygon>
							<polygon points="0 4.54747351e-13 50.3355705 4.54747351e-13 139.563949 185.944453 143.959732 300"></polygon>
						</clipPath>
					</defs>
					<rect clipPath="url(#Group)" id="Rectangle" fill="currentColor" x="0" y="0" width="300" height="300"></rect>
				</svg>

				<div className="flex space-x-4 items-center font-semibold text-white">
					<Suspense>
						<Await resolve={user} errorElement={
							<>
								<Link className="p-2" to="/login">Log In</Link>
								<Link className="p-2 px-4 bg-white/20 rounded-lg" to="/signup">Sign Up</Link>
							</>
						}>
							{_ => <Link to="/accounts">Dashboard</Link>}
						</Await>
					</Suspense>
				</div>
			</div>
			<div className="grid md:grid-cols-2 my-auto">
				<div className="">
					<div className="flex items-center space-x-4">
						<h1 className="text-2xl font-bold tracking-tigh text-purple-400">Ultrafinance</h1>
						<span className="rounded-full bg-gradient-to-r from-teal-500 to-cyan-600 px-3 py-0.5 text-sm text-white font-semibold outline-white/10 outline-4 outline">Early Alpha</span>
					</div>
					<h2 className="text-6xl text-white font-bold tracking-tight mt-6">Personal DevOps for <br />
						<span className="bg-gradient-to-r from-teal-200 to-cyan-400 bg-clip-text pb-3 text-transparent">your finances</span>.</h2>
					<p className="mt-8 text-base text-purple-200/80 sm:text-xl lg:text-lg xl:text-xl">Ultrafinance allows you to route your financial transactions, accounts and assets to automations and custom functions. Ultrafinance is built for power-users. Even write code in TypeScript &amp; JavaScript to get ultimate flexibility in automation.</p>
				</div>
			</div>
		</div>
		<div className="p-8 pt-20 text-center max-w-6xl mx-auto">
			<h4 className="text-lg font-semibold text-purple-600">Routing for your financial data</h4>
			<h3 className="mt-2 text-3xl font-bold tracking-tight text-gray-900">Connect accounts and import transactions</h3>
			<p className="mx-auto my-14 max-w-prose text-xl text-gray-500">Connect to over 2,000 financial institutions, bank accounts and credit cards to import your data. Your data is always encrypted in transite, at-rest and stored in the EU.</p>
			<img src={`${import.meta.env.BASE_URL}screenshots/accounts.png`} />
		</div>
		<div className="p-8 text-center max-w-6xl mx-auto">
			<h4 className="text-lg font-semibold text-purple-600">Automations ready to go</h4>
			<h3 className="mt-2 text-3xl font-bold tracking-tight text-gray-900">Integrate with pre-existing destinations</h3>
			<div className="grid lg:grid-cols-3 mt-20 gap-8">
				<div className="pt-6">
					<div className="flow-root rounded-lg bg-gray-50 px-6 pb-8">
						<div className="-mt-6">
							<div>
								<span className="inline-flex items-center justify-center rounded-md bg-gradient-to-r from-purple-500 to-purple-600 p-3 shadow-lg">
									<svg className="h-6 w-6 text-white" x-description="Heroicon name: outline/cloud-arrow-up" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor" aria-hidden="true">
										<path strokeLinecap="round" strokeLinejoin="round" d="M12 16.5V9.75m0 0l3 3m-3-3l-3 3M6.75 19.5a4.5 4.5 0 01-1.41-8.775 5.25 5.25 0 0110.233-2.33 3 3 0 013.758 3.848A3.752 3.752 0 0118 19.5H6.75z"></path>
									</svg>
								</span>
							</div>
							<h3 className="mt-8 text-lg font-medium tracking-tight text-gray-900">Lunch Money</h3>
							<p className="mt-5 text-base text-gray-500">Send transactions and account balances to the budgeting app <a className="text-purple-600" href="https://lunchmoney.app/">Lunch Money</a>.</p>
						</div>
					</div>
				</div>
				<div className="pt-6">
					<div className="flow-root rounded-lg bg-gray-50 px-6 pb-8">
						<div className="-mt-6">
							<div>
								<span className="inline-flex items-center justify-center rounded-md bg-gradient-to-r from-purple-500 to-purple-600 p-3 shadow-lg text-white">
									<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="w-6 h-6">
										<path strokeLinecap="round" strokeLinejoin="round" d="M21.75 6.75v10.5a2.25 2.25 0 01-2.25 2.25h-15a2.25 2.25 0 01-2.25-2.25V6.75m19.5 0A2.25 2.25 0 0019.5 4.5h-15a2.25 2.25 0 00-2.25 2.25m19.5 0v.243a2.25 2.25 0 01-1.07 1.916l-7.5 4.615a2.25 2.25 0 01-2.36 0L3.32 8.91a2.25 2.25 0 01-1.07-1.916V6.75" />
									</svg>
								</span>
							</div>
							<h3 className="mt-8 text-lg font-medium tracking-tight text-gray-900">Email</h3>
							<p className="mt-5 text-base text-gray-500">Get notified via email for specific transactions and events.</p>
						</div>
					</div>
				</div>
				<div className="pt-6">
					<div className="flow-root rounded-lg bg-gray-50 px-6 pb-8">
						<div className="-mt-6">
							<div>
								<span className="inline-flex items-center justify-center rounded-md bg-gradient-to-r from-purple-500 to-purple-600 p-3 shadow-lg text-white">
									<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="w-6 h-6">
										<path strokeLinecap="round" strokeLinejoin="round" d="M12.75 3.03v.568c0 .334.148.65.405.864l1.068.89c.442.369.535 1.01.216 1.49l-.51.766a2.25 2.25 0 01-1.161.886l-.143.048a1.107 1.107 0 00-.57 1.664c.369.555.169 1.307-.427 1.605L9 13.125l.423 1.059a.956.956 0 01-1.652.928l-.679-.906a1.125 1.125 0 00-1.906.172L4.5 15.75l-.612.153M12.75 3.031a9 9 0 00-8.862 12.872M12.75 3.031a9 9 0 016.69 14.036m0 0l-.177-.529A2.25 2.25 0 0017.128 15H16.5l-.324-.324a1.453 1.453 0 00-2.328.377l-.036.073a1.586 1.586 0 01-.982.816l-.99.282c-.55.157-.894.702-.8 1.267l.073.438c.08.474.49.821.97.821.846 0 1.598.542 1.865 1.345l.215.643m5.276-3.67a9.012 9.012 0 01-5.276 3.67m0 0a9 9 0 01-10.275-4.835M15.75 9c0 .896-.393 1.7-1.016 2.25" />
									</svg>

								</span>
							</div>
							<h3 className="mt-8 text-lg font-medium tracking-tight text-gray-900">Webhooks</h3>
							<p className="mt-5 text-base text-gray-500">Connect to any HTTP Webhook with JSON payloads of your transactions.</p>
						</div>
					</div>
				</div>
			</div>
			<p className="mt-6 text-gray-700 text-lg">And more coming soon...</p>
		</div>
		<div className="p-8 text-center bg-slate-100 mt-8">
			<div className="p-8 text-center max-w-6xl mx-auto">
				<h4 className="text-lg font-semibold text-purple-600">Get advanced</h4>
				<h3 className="mt-2 text-3xl font-bold tracking-tight text-gray-900">Write your own with JavaScript / TypeScript</h3>
				<p className="mx-auto my-14 max-w-prose text-xl text-gray-500">Use Ultrafinance <span className="px-2 py-1 rounded-lg bg-purple-100 text-purple-600">Cloud Functions</span> for ultimate flexibility and control. TypeScript functions run in a secure isolated sandbox (built on <a className="text-purple-600" href="https://deno.land/">Deno</a>) giving you the ultimate power: writing code.</p>
				<img src={`${import.meta.env.BASE_URL}screenshots/functions.png`} />
			</div>
		</div>
		<div className="p-8 text-center mt-8">
			<div className="p-8 text-center max-w-6xl mx-auto">
				<h4 className="text-lg font-semibold text-purple-600">Work in the open</h4>
				<h3 className="mt-2 text-3xl font-bold tracking-tight text-gray-900">Open Source</h3>
				<p className="mx-auto my-14 max-w-prose text-xl text-gray-500">Ultrafinance is 100% open source, available on <a className="text-purple-600" href="https://github.com/joehoyle/ultrafinance/">GitHub</a>. Even the ultrafinance.app <a className="text-purple-600" href="https://github.com/joehoyle/ultrafinance/tree/main/terraform">infrastructure</a> is open source. Ultrafinance is built with Rust for maximum performance and safety. Patches and feedback welcome!</p>
			</div>
		</div>
		<Footer />
	</>
}
